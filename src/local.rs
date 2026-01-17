use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use anyhow::Result;
use coarsetime::Instant;
use log::info;
use parking_lot::RwLock;
use tokio::sync::OnceCell;

pub const NULL_SOCKET_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_bits(0)), 0);

static CONNECTED: AtomicBool = AtomicBool::new(false);
static LOCAL_ADDR: RwLock<SocketAddr> = RwLock::new(NULL_SOCKET_ADDR);

static START: OnceCell<Instant> = OnceCell::const_new();
static LAST_SEEN: AtomicU64 = AtomicU64::new(0);

pub struct LocalAddr;

impl LocalAddr {
    #[inline(always)]
    pub fn current() -> SocketAddr {
        *LOCAL_ADDR.read()
    }

    #[inline(always)]
    fn set(addr: SocketAddr) {
        *LOCAL_ADDR.write() = addr
    }

    #[inline(always)]
    fn clear() {
        *LOCAL_ADDR.write() = NULL_SOCKET_ADDR
    }
}

pub struct ConnectCtx;

impl ConnectCtx {
    #[inline(always)]
    pub fn connect(addr: SocketAddr) {
        if ConnectCtx::try_connect() {
            LocalAddr::set(addr);
            info!("connected client {addr}");
        }
    }

    #[inline(always)]
    pub fn is_connected() -> bool {
        CONNECTED.load(Ordering::Acquire)
    }

    #[inline(always)]
    pub async fn disconnect() {
        CONNECTED.store(false, Ordering::Release);
        LocalAddr::clear();
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
    pub fn now() -> Result<()> {
        Ok(START.set(Instant::now())?)
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
        LAST_SEEN.store(Started::at().elapsed().as_millis(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn elapsed() -> Duration {
        Duration::from_millis(
            Started::at()
                .elapsed()
                .as_millis()
                .saturating_sub(LAST_SEEN.load(Ordering::Relaxed)),
        )
    }
}
