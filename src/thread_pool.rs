use anyhow::Result;
use rayon::ThreadPool;
use tokio::sync::OnceCell;

use crate::K;

static THREAD_POOL: OnceCell<ThreadPool> = OnceCell::const_new();

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

pub fn get() -> &'static ThreadPool {
    THREAD_POOL.get().unwrap()
}
