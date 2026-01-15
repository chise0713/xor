use std::ops::Deref;

use anyhow::Result;
use rayon::ThreadPool as TP;
use tokio::sync::OnceCell;

use crate::K;

static THREAD_POOL: OnceCell<TP> = OnceCell::const_new();

pub struct ThreadPool;

impl ThreadPool {
    pub fn init(compute_threads: usize) -> Result<()> {
        THREAD_POOL.set(
            rayon::ThreadPoolBuilder::new()
                .num_threads(compute_threads)
                .stack_size(32 * K)
                .thread_name(|i| format!("xor-worker-{}", i))
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
