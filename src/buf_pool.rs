use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    slice,
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use crossbeam_utils::CachePadded;
use tinystr::{TinyAsciiStr, tinystr};
use tokio::sync::{OnceCell, Semaphore as SP};
use wide::u64x8;

use crate::NOT_INITED;

static POOL_SEM: SP = SP::const_new(0);

pub const SIMD_WIDTH: usize = size_of::<u64x8>();

struct Raw {
    ptr: NonNull<u8>,
    padded_len: usize,
}

#[repr(transparent)]
pub struct AlignBox {
    inner: CachePadded<Raw>,
}

pub const CACHELINE_ALIGN: usize = size_of::<AlignBox>();

unsafe impl Send for AlignBox {}

impl AlignBox {
    pub fn new(size: usize) -> Self {
        let align_size = CACHELINE_ALIGN.max(SIMD_WIDTH);
        let padded_len = size.div_ceil(SIMD_WIDTH) * SIMD_WIDTH;
        let layout = Layout::from_size_align(padded_len, align_size).unwrap();
        let raw_ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self {
            inner: CachePadded::new(Raw { ptr, padded_len }),
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

pub struct BufPool;

impl BufPool {
    pub fn init(limit: usize, payload_max: usize) -> Result<()> {
        BUF_POOL.set(ArrayQueue::new(limit))?;

        (0..limit).for_each(|_| {
            BufPool
                .push(AlignBox::new(payload_max))
                .ok()
                .expect("failed to push BufPool")
        });

        Semaphore.add_permits(limit);
        Ok(())
    }
}

impl Deref for BufPool {
    type Target = ArrayQueue<AlignBox>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let fmt: TinyAsciiStr<32> = tinystr!(32, "BufPool").concat(NOT_INITED);
        let fmt = fmt.as_str();
        BUF_POOL.get().expect(fmt)
    }
}

pub struct Semaphore;

impl Deref for Semaphore {
    type Target = SP;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &POOL_SEM
    }
}
