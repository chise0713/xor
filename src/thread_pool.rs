use std::ops::Deref;

use anyhow::Result;
use rayon::{ThreadPool as TP, ThreadPoolBuilder};
use tokio::sync::OnceCell;

use crate::K;

const STACK_SIZE: usize = 128 * K;
static THREAD_POOL: OnceCell<TP> = OnceCell::const_new();
static SEEN_THREAD: OnceCell<TP> = OnceCell::const_new();

pub struct ThreadPool;

impl ThreadPool {
    pub fn build(compute_threads: usize) -> Result<()> {
        const SEEN_THREAD_COUNT: usize = 1;
        THREAD_POOL.set(
            ThreadPoolBuilder::new()
                .num_threads(compute_threads.saturating_sub(SEEN_THREAD_COUNT).max(1))
                .stack_size(STACK_SIZE)
                .thread_name(|i| format!("xor-worker-{}", i))
                .build()?,
        )?;
        SEEN_THREAD.set(
            ThreadPoolBuilder::new()
                .num_threads(SEEN_THREAD_COUNT)
                .stack_size(STACK_SIZE)
                .thread_name(|_| "seen-worker".to_string())
                .build()?,
        )?;
        Ok(())
    }
}

pub struct XorThreads;

impl Deref for XorThreads {
    type Target = TP;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let t = THREAD_POOL.get();
        debug_assert!(t.is_some());
        unsafe { t.unwrap_unchecked() }
    }
}

pub struct SeenThread;
impl Deref for SeenThread {
    type Target = TP;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let t = SEEN_THREAD.get();
        debug_assert!(t.is_some());
        unsafe { t.unwrap_unchecked() }
    }
}
