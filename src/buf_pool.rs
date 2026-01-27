use std::{
    alloc::{self, Layout},
    io::{Error, ErrorKind},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    slice,
    sync::OnceLock,
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use crossbeam_utils::CachePadded;
use log_limit::warn_limit_global;
use tokio::sync::{Semaphore as SP, TryAcquireError};
use wide::u64x8;

use self::sealed::{BpSealed, PoolInner};
use crate::{INIT, ONCE, WARN_LIMIT_DUR, const_concat};

pub const SIMD_WIDTH: usize = size_of::<u64x8>();

pub struct AlignBox {
    ptr: NonNull<u8>,
    padded_len: usize,
}

pub const CACHELINE_ALIGN: usize = size_of::<CachePadded<Box<()>>>();

impl AlignBox {
    #[inline(always)]
    pub fn align() -> usize {
        CACHELINE_ALIGN.max(SIMD_WIDTH)
    }

    #[inline(always)]
    fn padded_len(size: usize) -> usize {
        size.div_ceil(SIMD_WIDTH) * SIMD_WIDTH
    }

    #[inline(always)]
    fn layout(size: usize) -> Layout {
        Layout::from_size_align(Self::padded_len(size), Self::align()).unwrap()
    }

    pub fn new(size: usize) -> Self {
        let padded_len = Self::padded_len(size);
        let layout = Self::layout(size);
        let raw_ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self { ptr, padded_len }
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.padded_len, Self::align()).unwrap();
        unsafe { alloc::dealloc(self.ptr.as_ptr(), layout) }
    }
}

impl Deref for AlignBox {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.padded_len) }
    }
}

impl DerefMut for AlignBox {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.padded_len) }
    }
}

static BUF_POOL: OnceLock<PoolInner> = OnceLock::new();

const PUSH_FAILURE: &str = "failed to push BufPool";

pub struct BufPool;

impl BufPool {
    pub fn init(limit: usize, payload_max: usize) -> Result<()> {
        let stride = AlignBox::padded_len(payload_max);
        let slab = AlignBox::new(limit * stride);

        if BUF_POOL.set(PoolInner::new(limit, slab, stride)).is_err() {
            return Err(Error::new(ErrorKind::AlreadyExists, ONCE.as_str()))?;
        };

        (0..limit).for_each(|i| {
            BpSealed.push(i).expect(PUSH_FAILURE);
        });

        Semaphore.add_permits(limit);
        Ok(())
    }

    pub async fn acquire() -> Option<LeasedBuf> {
        match Semaphore.try_acquire() {
            Ok(permit) => permit,
            Err(TryAcquireError::Closed) => {
                return None;
            }
            Err(TryAcquireError::NoPermits) => {
                warn_limit_global!(1, WARN_LIMIT_DUR, "semaphore backpressure");
                Semaphore.acquire().await.ok()?
            }
        }
        .forget();

        BpSealed.pop()
    }
}

pub struct LeasedBuf {
    ptr: NonNull<u8>,
    meta: usize,
}

unsafe impl Send for LeasedBuf {}

impl Deref for LeasedBuf {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), BpSealed.stride()) }
    }
}

impl DerefMut for LeasedBuf {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), BpSealed.stride()) }
    }
}

impl Drop for LeasedBuf {
    fn drop(&mut self) {
        BpSealed.push(self.meta).expect(PUSH_FAILURE);
        Semaphore.add_permits(1);
    }
}

mod sealed {
    use super::*;
    pub(super) struct BpSealed;

    impl Deref for BpSealed {
        type Target = PoolInner;

        #[inline(always)]
        fn deref(&self) -> &Self::Target {
            const_concat! {
                CTX = "BufPool: " + INIT
            };
            BUF_POOL.get().expect(&CTX)
        }
    }

    pub(super) struct PoolInner {
        available: ArrayQueue<usize>,
        slab: AlignBox,
        stride: usize,
    }

    impl PoolInner {
        pub(super) fn new(available: usize, slab: AlignBox, stride: usize) -> Self {
            Self {
                available: ArrayQueue::new(available),
                slab,
                stride,
            }
        }

        pub(super) fn push(&self, meta: usize) -> Result<(), usize> {
            self.available.push(meta)
        }

        pub(super) fn pop(&self) -> Option<LeasedBuf> {
            let meta = self.available.pop()?;
            let offset = meta * self.stride;
            let ptr = NonNull::new(unsafe { self.slab.as_ptr().add(offset).cast_mut() })?;
            Some(LeasedBuf { ptr, meta })
        }

        #[inline(always)]
        pub(super) fn stride(&self) -> usize {
            self.stride
        }
    }

    unsafe impl Send for PoolInner {}
    unsafe impl Sync for PoolInner {}
}

static POOL_SEM: SP = SP::const_new(0);

struct Semaphore;

impl Deref for Semaphore {
    type Target = SP;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &POOL_SEM
    }
}
