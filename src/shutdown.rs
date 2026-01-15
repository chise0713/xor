use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn set(val: bool) {
    SHUTDOWN.store(val, Ordering::Relaxed)
}

pub fn cmp_exchange(current: bool, new: bool) -> bool {
    SHUTDOWN
        .compare_exchange(current, new, Ordering::Release, Ordering::Acquire)
        .is_ok()
}

pub fn shutdown() -> bool {
    SHUTDOWN.load(Ordering::Relaxed)
}
