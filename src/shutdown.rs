use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub struct Shutdown;

impl Shutdown {
    #[inline(always)]
    pub fn request() {
        SHUTDOWN.store(true, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn try_request() -> bool {
        SHUTDOWN
            .compare_exchange(false, true, Ordering::Release, Ordering::Acquire)
            .is_ok()
    }

    #[inline(always)]
    pub fn requested() -> bool {
        SHUTDOWN.load(Ordering::Relaxed)
    }
}
