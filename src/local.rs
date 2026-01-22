use std::{
    io::{Error, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Result;
use coarsetime::Instant;
use log::info;
use parking_lot::{Once, OnceState, RwLock};

use crate::{INIT, K, ONCE, concat_let};

const NULL_SOCKET_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_bits(0)), 0);

static LOCAL_ADDR: RwLock<SocketAddr> = RwLock::new(NULL_SOCKET_ADDR);

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

static CONNECTED: AtomicBool = AtomicBool::new(false);

pub struct ConnectCtx;

impl ConnectCtx {
    #[inline(always)]
    pub fn connect(addr: SocketAddr) {
        if ConnectCtx::try_connect() {
            LastSeen::now();
            LocalAddr::set(addr);
            WatchDog::unpark();
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

static START: OnceLock<Instant> = OnceLock::new();

pub struct Started;

impl Started {
    #[inline(always)]
    pub fn now() {
        concat_let! {
           ctx = "Socket::now(): " + ONCE
        };
        START.set(Instant::now()).expect(&ctx)
    }

    #[inline(always)]
    fn at() -> Instant {
        concat_let! {
           ctx = "Socket::at(): " + INIT
        };
        *START.get().expect(&ctx)
    }
}

static UPDATE_INTERVAL: AtomicU64 = AtomicU64::new(500);

static LAST_SEEN: AtomicU64 = AtomicU64::new(0);

pub struct LastSeen;

impl LastSeen {
    #[inline(always)]
    pub fn now() {
        let current_time = Started::at().elapsed().as_millis();
        let last = LAST_SEEN.load(Ordering::Relaxed);
        if current_time > last + UPDATE_INTERVAL.load(Ordering::Relaxed) {
            LAST_SEEN.store(current_time, Ordering::Relaxed);
        }
    }

    #[inline(always)]
    fn elapsed() -> Duration {
        Duration::from_millis(
            Started::at()
                .elapsed()
                .as_millis()
                .saturating_sub(LAST_SEEN.load(Ordering::Relaxed)),
        )
    }
}

static WATCH_DOG_HANDLE: OnceLock<JoinHandle<()>> = OnceLock::new();
static WATCH_DOG: Once = Once::new();

pub struct WatchDog;

impl WatchDog {
    pub fn start(timeout: f64) -> Result<()> {
        concat_let! {
            ctx = "WatchDog::start()" + ONCE
        };
        if matches!(WATCH_DOG.state(), OnceState::Done) {
            return Err(Error::new(ErrorKind::AlreadyExists, ctx.as_str()))?;
        };
        WATCH_DOG.call_once(|| {});
        WATCH_DOG_HANDLE
            .set(
                thread::Builder::new()
                    .name("clnt-wdog".to_string())
                    .stack_size(16 * K)
                    .spawn(move || watchdog(timeout))?,
            )
            .expect(&ctx);
        Ok(())
    }

    #[inline(always)]
    fn unpark() {
        concat_let! {
            ctx = "WatchDog::unpark()" + INIT
        };
        WATCH_DOG_HANDLE.get().expect(&ctx).thread().unpark()
    }
}

#[inline(always)]
fn watchdog(timeout: f64) {
    let timeout_dur = Duration::from_secs_f64(timeout);
    let park_dur = Duration::from_secs_f64(timeout.div_euclid(3.));
    UPDATE_INTERVAL.store((park_dur / 2).as_millis() as u64, Ordering::Relaxed);

    loop {
        let park_dur = if ConnectCtx::is_connected() {
            park_dur
        } else {
            Duration::MAX
        };

        thread::park_timeout(park_dur);

        if LastSeen::elapsed() < timeout_dur {
            continue;
        }

        let addr = LocalAddr::current();
        ConnectCtx::disconnect();
        info!("client timeout {addr}");
    }
}
