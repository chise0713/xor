use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn set(val: bool) {
    SHUTDOWN.store(val, Ordering::Relaxed)
}

pub fn cmp_exchange(a: bool, b: bool) -> bool {
    SHUTDOWN
        .compare_exchange(a, b, Ordering::Release, Ordering::Acquire)
        .is_ok()
}

pub fn shutdown() -> bool {
    SHUTDOWN.load(Ordering::Relaxed)
}
