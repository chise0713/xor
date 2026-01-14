use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use tokio::sync::OnceCell;

use crate::{POOL_SEM, xor::AlignBox};

pub static BUF_POOL: OnceCell<ArrayQueue<AlignBox>> = OnceCell::const_new();

pub fn init(limit: usize, payload_max: usize) -> Result<()> {
    BUF_POOL.set(ArrayQueue::new(limit))?;
    (0..limit).for_each({
        let pool = BUF_POOL.get().unwrap();
        |_| pool.push(AlignBox::new(payload_max)).unwrap()
    });
    POOL_SEM.add_permits(limit);
    Ok(())
}
