use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use coarsetime::Instant;
use log::info;
use parking_lot::RwLock;
use tinystr::{TinyAsciiStr, tinystr};

use crate::{NOT_INITED, ONCE};

pub const NULL_SOCKET_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_bits(0)), 0);

static CONNECTED: AtomicBool = AtomicBool::new(false);
static LOCAL_ADDR: RwLock<SocketAddr> = RwLock::new(NULL_SOCKET_ADDR);

static START: OnceLock<Instant> = OnceLock::new();
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
    pub fn disconnect() {
        CONNECTED.store(false, Ordering::Release);
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
    pub fn now() {
        let fmt: TinyAsciiStr<32> = tinystr!(32, "Started").concat(ONCE);
        let fmt = fmt.as_str();
        START.set(Instant::now()).expect(fmt)
    }

    #[inline(always)]
    fn at() -> Instant {
        let fmt: TinyAsciiStr<32> = tinystr!(32, "Started").concat(NOT_INITED);
        let fmt = fmt.as_str();
        *START.get().expect(fmt)
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
