use std::{
    io::{Error, ErrorKind},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Result;
use coarsetime::Instant;
use log::info;
use parking_lot::RwLock;

use crate::{CORASETIME_UPDATE, INIT, K, ONCE, const_concat};

pub const NULL_SOCKET_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::from_bits(0)), 0);

static LOCAL_ADDR: RwLock<SocketAddr> = RwLock::new(NULL_SOCKET_ADDR);

static LOCAL_ADDR_VERSION: AtomicUsize = AtomicUsize::new(0);

pub struct LocalAddr;

impl LocalAddr {
    #[must_use]
    #[inline]
    pub fn version() -> usize {
        LOCAL_ADDR_VERSION.load(Ordering::Acquire)
    }

    /// returns `false` when `glob_ver` equals to `ver`
    #[must_use]
    #[inline]
    pub fn check_and_update(ver: &mut usize) -> bool {
        let glob_ver = Self::version();
        if *ver != glob_ver {
            *ver = glob_ver;
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn current() -> SocketAddr {
        *LOCAL_ADDR.read()
    }

    fn set(addr: SocketAddr) {
        *LOCAL_ADDR.write() = addr;
        LOCAL_ADDR_VERSION.fetch_add(1, Ordering::Release);
    }
}

static CONNECTED: AtomicBool = AtomicBool::new(false);

pub struct ConnectCtx;

impl ConnectCtx {
    #[cold]
    #[inline(never)]
    pub fn connect(addr: SocketAddr) {
        if ConnectCtx::try_connect() {
            LocalAddr::set(addr);
            LastSeen::now();
            WatchDog::unpark();
            info!("connected client {addr}");
        }
    }

    #[inline]
    pub fn is_connected() -> bool {
        CONNECTED.load(Ordering::Acquire)
    }

    fn disconnect() {
        CONNECTED.store(false, Ordering::Release);
    }

    fn try_connect() -> bool {
        CONNECTED
            .compare_exchange(false, true, Ordering::Release, Ordering::Acquire)
            .is_ok()
    }
}

static START: OnceLock<Instant> = OnceLock::new();

pub struct Started;

impl Started {
    pub fn now() -> Result<()> {
        let exist = |_| {
            const_concat! {
               CTX = "Socket::now(): " + ONCE
            };
            Error::new(ErrorKind::AlreadyExists, CTX.as_str())
        };
        START.set(Instant::now()).map_err(exist)?;
        Ok(())
    }

    fn at() -> Instant {
        const_concat! {
           CTX = "Socket::at(): " + INIT
        };
        *START.get().expect(&CTX)
    }
}

static UPDATE_INTERVAL: AtomicU64 = AtomicU64::new(CORASETIME_UPDATE);

static LAST_SEEN: AtomicU64 = AtomicU64::new(0);

pub struct LastSeen;

impl LastSeen {
    pub fn now() {
        let current_time = Started::at().elapsed().as_millis();
        let last = LAST_SEEN.load(Ordering::Relaxed);
        if current_time > last + UPDATE_INTERVAL.load(Ordering::Relaxed) {
            LAST_SEEN.store(current_time, Ordering::Relaxed);
        }
    }

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

pub struct WatchDog;

impl WatchDog {
    pub fn start(timeout: f64) -> Result<()> {
        const_concat! {
            CTX = "WatchDog::start()" + ONCE
        };
        if WATCH_DOG_HANDLE.get().is_some() {
            return Err(Error::new(ErrorKind::AlreadyExists, CTX.as_str()))?;
        };
        WATCH_DOG_HANDLE
            .set(
                thread::Builder::new()
                    .name("clnt-wdog".to_string())
                    .stack_size(16 * K)
                    .spawn(move || watchdog(timeout))?,
            )
            .expect(&CTX);
        Ok(())
    }

    fn unpark() {
        const_concat! {
            CTX = "WatchDog::unpark()" + INIT
        };
        WATCH_DOG_HANDLE.get().expect(&CTX).thread().unpark()
    }
}

fn watchdog(timeout: f64) {
    let timeout_dur = Duration::from_secs_f64(timeout);
    let park_dur = Duration::from_secs_f64(timeout.div_euclid(3.));
    UPDATE_INTERVAL.fetch_max(park_dur.as_millis().div_ceil(2) as u64, Ordering::Relaxed);

    loop {
        if ConnectCtx::is_connected() {
            thread::park_timeout(park_dur);
        } else {
            thread::park();
        };

        if LastSeen::elapsed() < timeout_dur {
            continue;
        }

        let addr = LocalAddr::current();
        ConnectCtx::disconnect();
        info!("client timeout {addr}");
    }
}
