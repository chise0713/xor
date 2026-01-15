use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

use tokio::sync::OnceCell;

static CONNECTED: AtomicBool = AtomicBool::new(false);
static LAST_SEEN: AtomicU64 = AtomicU64::new(0);
static START: OnceCell<Instant> = OnceCell::const_new();

pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

pub fn set_connected(val: bool) {
    CONNECTED.store(val, Ordering::Relaxed)
}

pub fn cmp_exchange_connected(current: bool, new: bool) -> bool {
    CONNECTED
        .compare_exchange(current, new, Ordering::Release, Ordering::Acquire)
        .is_ok()
}

pub fn init_started_at() {
    START.set(Instant::now()).unwrap()
}

pub fn started_at() -> Instant {
    *START.get().unwrap()
}

pub fn last_seen() -> Duration {
    Duration::from_millis(LAST_SEEN.load(Ordering::Relaxed))
}

pub fn set_last_seen(val: Instant) {
    LAST_SEEN.store(
        val.duration_since(*START.get().unwrap()).as_millis() as u64,
        Ordering::Relaxed,
    )
}
