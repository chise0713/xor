use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub struct Shutdown;

impl Shutdown {
    pub fn request() {
        SHUTDOWN.store(true, Ordering::Relaxed);
    }

    pub fn try_request() -> bool {
        SHUTDOWN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    pub fn requested() -> bool {
        SHUTDOWN.load(Ordering::Relaxed)
    }
}
