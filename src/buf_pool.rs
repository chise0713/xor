use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    slice, thread,
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use crossbeam_utils::CachePadded;
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelIterator as _, ParallelIterator as _},
};
use tokio::sync::{OnceCell, Semaphore as SP};
use wide::u64x8;

static POOL_SEM: SP = SP::const_new(0);

pub const SIMD_WIDTH: usize = size_of::<u64x8>();

#[derive(Debug)]
struct Raw {
    ptr: NonNull<u8>,
    padded_len: usize,
}

#[derive(Debug)]
pub struct AlignBox(CachePadded<Raw>);

pub const CACHELINE_ALIGN: usize = size_of::<AlignBox>();

unsafe impl Send for AlignBox {}

impl AlignBox {
    pub fn new(size: usize) -> Self {
        let align_size = CACHELINE_ALIGN.max(SIMD_WIDTH);
        let padded_len = size.div_ceil(SIMD_WIDTH) * SIMD_WIDTH;
        let layout = Layout::from_size_align(padded_len, align_size).unwrap();
        let raw_ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self(CachePadded::new(Raw { ptr, padded_len }))
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        let align_size = CACHELINE_ALIGN.max(SIMD_WIDTH);
        let layout = Layout::from_size_align(self.0.padded_len, align_size).unwrap();
        unsafe {
            alloc::dealloc(self.0.ptr.as_ptr(), layout);
        }
    }
}

impl Deref for AlignBox {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AlignBox {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Deref for Raw {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.padded_len) }
    }
}

impl DerefMut for Raw {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.padded_len) }
    }
}

static BUF_POOL: OnceCell<ArrayQueue<AlignBox>> = OnceCell::const_new();

pub struct BufPool;

impl BufPool {
    pub fn init(limit: usize, payload_max: usize) -> Result<()> {
        BUF_POOL.set(ArrayQueue::new(limit))?;

        let temp_tp = ThreadPoolBuilder::new()
            .num_threads(thread::available_parallelism()?.get())
            .thread_name(|i| format!("buf-init-{}", i))
            .build()?;

        temp_tp.install(|| {
            (0..limit)
                .into_par_iter()
                .for_each(|_| BufPool.push(AlignBox::new(payload_max)).unwrap())
        });

        Semaphore.add_permits(limit);
        Ok(())
    }
}

impl Deref for BufPool {
    type Target = ArrayQueue<AlignBox>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let b = BUF_POOL.get();
        debug_assert!(b.is_some());
        unsafe { b.unwrap_unchecked() }
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
