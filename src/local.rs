use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, Instant},
};

use log::info;
use tokio::sync::{Mutex, OnceCell, SetError};

const NULL_SOCKET_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_bits(0)), 0);

static CONNECTED: AtomicBool = AtomicBool::new(false);
static LOCAL_ADDR: Mutex<SocketAddr> = Mutex::const_new(NULL_SOCKET_ADDR);

static LAST_SEEN: AtomicU64 = AtomicU64::new(0);
static START: OnceCell<Instant> = OnceCell::const_new();

pub struct LocalAddr;

impl LocalAddr {
    #[inline(always)]
    pub async fn current() -> SocketAddr {
        *LOCAL_ADDR.lock().await
    }

    #[inline(always)]
    async fn set(addr: SocketAddr) {
        *LOCAL_ADDR.lock().await = addr
    }

    #[inline(always)]
    async fn clear() {
        *LOCAL_ADDR.lock().await = NULL_SOCKET_ADDR
    }
}

pub struct ConnectCtx;

impl ConnectCtx {
    #[inline(always)]
    pub async fn connect(addr: SocketAddr) {
        if ConnectCtx::try_connect() {
            LocalAddr::set(addr).await;
            info!("set client addr to {addr}");
        }
    }

    #[inline(always)]
    pub fn is_connected() -> bool {
        CONNECTED.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub async fn disconnect() {
        CONNECTED.store(false, Ordering::Release);
        LocalAddr::clear().await;
    }

    #[inline(always)]
    pub fn try_connect() -> bool {
        CONNECTED
            .compare_exchange(false, true, Ordering::Release, Ordering::Acquire)
            .is_ok()
    }
}

pub struct Started;

impl Started {
    #[inline(always)]
    pub fn now() -> Result<(), SetError<Instant>> {
        START.set(Instant::now())
    }

    #[inline(always)]
    fn at() -> Instant {
        let s = START.get();
        debug_assert!(s.is_some());
        unsafe { *s.unwrap_unchecked() }
    }
}

pub struct LastSeen;

impl LastSeen {
    #[inline(always)]
    pub fn now() {
        LAST_SEEN.store(
            Instant::now().duration_since(Started::at()).as_millis() as u64,
            Ordering::Relaxed,
        )
    }

    #[inline(always)]
    pub fn elapsed() -> Duration {
        Started::at()
            .elapsed()
            .saturating_sub(Duration::from_millis(LAST_SEEN.load(Ordering::Relaxed)))
    }
}
