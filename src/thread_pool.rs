use std::ops::Deref;

use anyhow::Result;
use rayon::{ThreadPool as TP, ThreadPoolBuilder};
use tokio::sync::OnceCell;

use crate::K;

const STACK_SIZE: usize = 256 * K;

static THREAD_POOL: OnceCell<TP> = OnceCell::const_new();

pub struct ThreadPool;

impl ThreadPool {
    pub fn build(threads: usize) -> Result<()> {
        THREAD_POOL.set(
            ThreadPoolBuilder::new()
                .num_threads(threads)
                .stack_size(STACK_SIZE)
                .thread_name(|i| format!("worker-{}", i))
                .build()?,
        )?;
        Ok(())
    }
}

impl Deref for ThreadPool {
    type Target = TP;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let t = THREAD_POOL.get();
        debug_assert!(t.is_some());
        unsafe { t.unwrap_unchecked() }
    }
}
