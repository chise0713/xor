use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    slice,
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use crossbeam_utils::CachePadded;
use log_limit::warn_limit_global;
use tokio::sync::{OnceCell, Semaphore as SP, SemaphorePermit, TryAcquireError};
use wide::u64x8;

use crate::{INIT, WARN_LIMIT_DUR, buf_pool::sealed::BpSealed, concat_let};

static POOL_SEM: SP = SP::const_new(0);

pub const SIMD_WIDTH: usize = size_of::<u64x8>();

struct Inner {
    ptr: NonNull<u8>,
    padded_len: usize,
}

#[repr(transparent)]
struct AlignBox {
    inner: CachePadded<Inner>,
}

pub const CACHELINE_ALIGN: usize = size_of::<AlignBox>();

unsafe impl Send for AlignBox {}

impl AlignBox {
    fn new(size: usize) -> Self {
        let align_size = CACHELINE_ALIGN.max(SIMD_WIDTH);
        let padded_len = size.div_ceil(SIMD_WIDTH) * SIMD_WIDTH;
        let layout = Layout::from_size_align(padded_len, align_size).unwrap();
        let raw_ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self {
            inner: CachePadded::new(Inner { ptr, padded_len }),
        }
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        let align_size = CACHELINE_ALIGN.max(SIMD_WIDTH);
        let layout = Layout::from_size_align(self.inner.padded_len, align_size).unwrap();
        unsafe {
            alloc::dealloc(self.inner.ptr.as_ptr(), layout);
        }
    }
}

impl Deref for AlignBox {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.inner.ptr.as_ptr(), self.inner.padded_len) }
    }
}

impl DerefMut for AlignBox {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.inner.ptr.as_ptr(), self.inner.padded_len) }
    }
}

static BUF_POOL: OnceCell<ArrayQueue<AlignBox>> = OnceCell::const_new();
const PUSH_FAILURE: &str = "failed to push BufPool";

pub struct BufPool;

impl BufPool {
    pub fn init(limit: usize, payload_max: usize) -> Result<()> {
        BUF_POOL.set(ArrayQueue::new(limit))?;

        (0..limit).for_each(|_| {
            BpSealed
                .push(AlignBox::new(payload_max))
                .ok()
                .expect(PUSH_FAILURE)
        });

        Semaphore.add_permits(limit);
        Ok(())
    }

    pub async fn acquire() -> Option<LeasedBuf> {
        let _p = match Semaphore.try_acquire() {
            Ok(_p) => _p,
            Err(TryAcquireError::Closed) => {
                return None;
            }
            Err(TryAcquireError::NoPermits) => {
                warn_limit_global!(1, WARN_LIMIT_DUR, "semaphore backpressure");
                Semaphore.acquire().await.unwrap()
            }
        };
        let buf = BpSealed.pop().expect("semaphore permits mismatch!");

        Some(LeasedBuf {
            inner: Some(buf),
            _p,
        })
    }
}

pub struct LeasedBuf {
    // AlignBox is too large,
    // Option<T> is just for take
    inner: Option<AlignBox>,
    _p: SemaphorePermit<'static>,
}

impl Deref for LeasedBuf {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.inner.as_deref().unwrap()
    }
}

impl DerefMut for LeasedBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_deref_mut().unwrap()
    }
}

impl Drop for LeasedBuf {
    fn drop(&mut self) {
        BpSealed
            .push(self.inner.take().unwrap())
            .ok()
            .expect(PUSH_FAILURE);
    }
}

mod sealed {
    use super::*;
    pub(super) struct BpSealed;

    impl Deref for BpSealed {
        type Target = ArrayQueue<AlignBox>;

        #[inline(always)]
        fn deref(&self) -> &Self::Target {
            concat_let!(ctx = "BufPool: " + INIT);
            BUF_POOL.get().expect(&ctx)
        }
    }
}

struct Semaphore;

impl Deref for Semaphore {
    type Target = SP;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &POOL_SEM
    }
}
