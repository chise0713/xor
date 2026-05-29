use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub struct Shutdown;

impl Shutdown {
    #[inline]
    pub fn request() {
        SHUTDOWN.store(true, Ordering::Relaxed);
    }

    #[inline]
    pub fn try_request() -> bool {
        SHUTDOWN
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    pub fn requested() -> bool {
        SHUTDOWN.load(Ordering::Relaxed)
    }
}
