use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    thread,
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use rayon::{
    ThreadPoolBuilder,
    iter::{IntoParallelIterator as _, ParallelIterator as _},
};
use tokio::sync::{OnceCell, Semaphore as SP, SetError};

static POOL_SEM: SP = SP::const_new(0);

const BUF_ALIGN: usize = 64;

#[derive(Debug)]
#[repr(align(32))]
pub struct AlignBox {
    ptr: NonNull<u8>,
    len: usize,
    layout: Layout,
}

unsafe impl Send for AlignBox {}
unsafe impl Sync for AlignBox {}

impl AlignBox {
    pub fn new(size: usize) -> Self {
        let layout = Layout::from_size_align(size, BUF_ALIGN).unwrap();
        unsafe {
            let raw_ptr = alloc::alloc(layout);
            let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
            Self {
                ptr,
                len: size,
                layout,
            }
        }
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        unsafe {
            alloc::dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}

impl Deref for AlignBox {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for AlignBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

static BUF_POOL: OnceCell<ArrayQueue<AlignBox>> = OnceCell::const_new();

pub struct BufPool;

impl BufPool {
    pub fn init(limit: usize, payload_max: usize) -> Result<()> {
        BUF_POOL.set(ArrayQueue::new(limit))?;

        (0..limit).for_each(|_| BufPool.push(AlignBox::new(payload_max)).unwrap());

        POOL_SEM.add_permits(limit);
        Ok(())
    }

    pub fn init_parallel(limit: usize, payload_max: usize) -> Result<()> {
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

        let bufs = temp_tp.install(|| {
            let bufs: Box<[_]> = (0..limit)
                .into_par_iter()
                .map(|_| AlignBox::new(payload_max))
                .collect();
            bufs
        });
        for buf in bufs {
            BufPool.push(buf).unwrap();
        }

        POOL_SEM.add_permits(limit);
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
