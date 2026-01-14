use std::{
    sync::atomic::{AtomicBool, AtomicU64},
    time::Instant,
};

use tokio::sync::OnceCell;

pub static CONNECTED: AtomicBool = AtomicBool::new(false);
pub static LAST_SEEN: AtomicU64 = AtomicU64::new(0);
pub static START: OnceCell<Instant> = OnceCell::const_new();
