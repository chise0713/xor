use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use coarsetime::Instant;
use log::info;
use parking_lot::RwLock;

use crate::{INIT, K, ONCE, concat_let};

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
        concat_let!(ctx = "Socket::now(): " + ONCE);
        START.set(Instant::now()).expect(&ctx)
    }

    #[inline(always)]
    fn at() -> Instant {
        concat_let!(ctx = "Socket::at(): " + INIT);
        *START.get().expect(&ctx)
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

pub struct WatchDog;

impl WatchDog {
    pub fn init(timeout: f64) -> Result<()> {
        thread::Builder::new()
            .name("clnt-wdog".to_string())
            .stack_size(16 * K)
            .spawn(move || watchdog(timeout))?;
        Ok(())
    }
}

#[inline(always)]
fn watchdog(timeout: f64) {
    let timeout_dur = Duration::from_secs_f64(timeout);
    let sleep_dur = Duration::from_secs_f64(timeout);

    loop {
        thread::sleep(sleep_dur);
        if !ConnectCtx::is_connected() {
            continue;
        }

        if LastSeen::elapsed() < timeout_dur {
            continue;
        }

        let addr = LocalAddr::current();
        ConnectCtx::disconnect();
        info!("client timeout {addr}");
    }
}
