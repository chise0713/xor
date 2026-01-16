use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    slice, thread,
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelIterator as _, ParallelIterator as _},
};
use tokio::sync::{OnceCell, Semaphore as SP, SetError};

static POOL_SEM: SP = SP::const_new(0);

// https://rust-lang.github.io/hashbrown/src/crossbeam_utils/cache_padded.rs.html#128-130
pub const CACHELINE_ALIGN: usize = {
    #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
    ))]
    {
        128
    }
    #[cfg(any(
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "riscv64",
    ))]
    {
        32
    }
    #[cfg(target_arch = "s390x")]
    {
        256
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips64",
        target_arch = "riscv64",
        target_arch = "s390x",
    )))]
    {
        64
    }
};

#[derive(Debug)]
pub struct AlignBox {
    ptr: NonNull<u8>,
    len: usize,
}

unsafe impl Send for AlignBox {}
unsafe impl Sync for AlignBox {}

impl AlignBox {
    pub fn new(len: usize) -> Self {
        let layout = Layout::from_size_align(len, CACHELINE_ALIGN).unwrap();
        let raw_ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self { ptr, len }
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.len, CACHELINE_ALIGN).unwrap();
        unsafe {
            alloc::dealloc(self.ptr.as_ptr(), layout);
        }
    }
}

impl Deref for AlignBox {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for AlignBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

static BUF_POOL: OnceCell<ArrayQueue<AlignBox>> = OnceCell::const_new();

pub struct BufPool;

impl BufPool {
    pub fn init(limit: usize, payload_max: usize) -> Result<()> {
        match BUF_POOL.set(ArrayQueue::new(limit)) {
            Ok(()) => {}
            Err(e) => {
                if !matches!(e, SetError::AlreadyInitializedError(_)) {
                    Err(e)?;
                }
            }
        };

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
