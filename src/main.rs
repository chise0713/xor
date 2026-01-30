#![cfg_attr(feature = "bench", feature(test))]

mod args;
mod buf_pool;
mod local;
mod logger;
mod methods;
mod recv_send;
mod shutdown;
mod socket;

use std::{
    num::NonZero,
    process::ExitCode,
    str::FromStr as _,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use anyhow::Result;
use coarsetime::Updater as BgClock;
use log::{Level, error, info, log_enabled, trace};
use tokio::{
    runtime::{Builder, Runtime},
    signal,
    task::{JoinSet, LocalSet},
};

use crate::{
    args::{Args, Parse as _},
    buf_pool::BufPool,
    local::{Started, WatchDog},
    logger::Logger,
    methods::{Method, MethodState, dns_pad, xor::XorToken},
    recv_send::RecvSend,
    shutdown::Shutdown,
    socket::Sockets,
};

macro_rules! tinystr_const {
    { $($name:ident = $str:literal),* $(,)? } => {
        use ::tinystr::TinyAsciiStr as TAS;
        $(
            const $name: TAS<{ $str.len() }> = {
                match TAS::try_from_str($str) {
                    Ok(s) => s,
                    Err(_) => panic!(concat!("failed to construct tinystr from \"", $str, "\"")),
                }
            };
        )*
    }
}

tinystr_const! {
    ONCE = "called once before",
    INIT = "not initialzed before"
}

#[macro_export]
macro_rules! const_concat {
    { $($name:ident = $prefix:literal + $suffix:expr),* $(,)? } => {
        use ::tinystr::TinyAsciiStr as TAS;
        $(
            const $name: TAS<{ $prefix.len() + $suffix.len() }> = {
                // all as `const` to avoid runtime computation
                const SUFFIX: TAS<{ $suffix.len() }> = $suffix;
                const BASE: TAS<{ $prefix.len() }> = match TAS::try_from_str($prefix) {
                    Ok(s) => s,
                    Err(_) => panic!(concat!(
                        "failed to construct tinystr from \"",
                        $prefix,
                        "\""
                    )),
                };
                BASE.concat(SUFFIX)
            };
        )*
    };
}

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;

const K: usize = 2usize.pow(10); // 1024
const M: usize = K.pow(2); // 1024 * 1024

const DEFAULT_MTU: usize = 1500;
const DEFAULT_BUFFER_LIMIT: usize = 48;

const CORASETIME_UPDATE: u64 = 100;
const WORKER_STACK_SIZE: usize = 256 * K;

const WARN_LIMIT_DUR: Duration = Duration::from_millis(250);

fn main() -> Result<ExitCode> {
    // start parsing
    let Args {
        buffer_limit_usize,
        listen_address,
        mtu_usize,
        remote_address,
        timeout_f64_secs,
        token_hex_u8,
        set_method,
    } = match Args::parse() {
        Ok(v) => v,
        Err(e) => {
            return Ok(e);
        }
    };

    let Some(listen_address) = listen_address else {
        return args::invalid_argument();
    };
    let Some(remote_address) = remote_address else {
        return args::invalid_argument();
    };

    let timeout = timeout_f64_secs.unwrap_or(5.).max(0.5);

    let mtu = mtu_usize.unwrap_or(DEFAULT_MTU);
    if mtu > LINK_MTU_MAX {
        return args::invalid_argument();
    }

    let payload_max = mtu.saturating_sub(LINK_PAYLOAD_OFFSET);
    let limit = buffer_limit_usize.unwrap_or(usize::MIN);

    let Ok(method) = set_method
        .as_deref()
        .map(Method::from_str)
        .transpose()
        .map(Option::unwrap_or_default)
    else {
        return args::invalid_argument();
    };

    match method {
        Method::Xor => {
            let Some(token) = token_hex_u8.and_then(|t| {
                t.strip_prefix("0x")
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
            }) else {
                return args::invalid_argument();
            };
            if XorToken::init(token).is_err() {
                return args::invalid_argument();
            };
        }

        Method::DnsPad => {
            if !dns_pad::payload_bound_check(payload_max) {
                return args::invalid_argument();
            }
        }

        Method::DnsUnPad => {}
    };
    // end parsing

    let (async_main, rt) = match AsyncMain::init(
        method,
        listen_address,
        remote_address,
        timeout,
        limit,
        payload_max,
    ) {
        Ok(v) => v,
        Err(e) => {
            error!("{e}");
            return Ok(ExitCode::FAILURE);
        }
    };

    rt.block_on(async_main.enter())
}

static N: AtomicUsize = AtomicUsize::new(0);

struct AsyncMain {
    sockets: Sockets,
    worker_threads: usize,
}

impl AsyncMain {
    fn init(
        method: Method,
        listen_address: Box<str>,
        remote_address: Box<str>,
        timeout: f64,
        limit: usize,
        payload_max: usize,
    ) -> Result<(Self, Runtime)> {
        Logger::init();

        info!("initializing..");
        MethodState::init(method)?;

        const MAIN_THREAD: usize = 1;
        // zero worker when only main thread available
        let total_threads = thread::available_parallelism()
            .map(NonZero::get)
            .unwrap_or_default();
        let worker_threads = total_threads.saturating_sub(MAIN_THREAD);
        let rt = if worker_threads == 0 {
            Builder::new_current_thread().enable_all().build()
        } else {
            Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(WORKER_STACK_SIZE)
                .thread_name_fn(move || {
                    static ID: AtomicUsize = AtomicUsize::new(0);
                    let id = ID.fetch_add(1, Ordering::SeqCst);
                    format!("{method}-{id}")
                })
                .worker_threads(worker_threads)
                .build()
        }?;

        let sockets = Sockets::new(&listen_address, &remote_address)?;
        BgClock::new(CORASETIME_UPDATE).start()?;
        WatchDog::start(timeout)?;
        BufPool::init(
            match (limit, total_threads) {
                // explicit limit disables automatic safety heuristics
                (l, _) if l != 0 => l,
                // one thread => two tasks => two permits
                (0, t) if t != 0 => t * 2,
                // defaults to `DEFAULT_BUFFER_LIMIT`
                _ => DEFAULT_BUFFER_LIMIT,
            },
            payload_max,
        )?;
        Started::now()?;

        Ok((
            Self {
                sockets,
                worker_threads,
            },
            rt,
        ))
    }

    #[inline(always)]
    async fn enter(self) -> Result<ExitCode> {
        use crate::recv_send::mode::{Inbound, Outbound};

        self.sockets.convert()?;

        let mut join_set = JoinSet::new();
        (0..self.worker_threads).for_each(|_| {
            join_set.spawn(RecvSend.recv::<Outbound>());
            join_set.spawn(RecvSend.recv::<Inbound>());
        });
        let local_set = LocalSet::new();
        join_set.spawn_local_on(RecvSend.recv::<Outbound>(), &local_set);

        info!("service started");

        let mut exit_code = ExitCode::FAILURE;
        let mut net_fail = false;

        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutting down");
                Shutdown::request();
                exit_code = ExitCode::SUCCESS;
            },
            _ = join_set.join_next() => {
                net_fail = true;
            },
            _ = local_set.run_until(RecvSend.recv::<Inbound>()) => {
                net_fail = true;
            }
        }
        join_set.abort_all();
        if net_fail {
            error!("a network recv task exited prematurely");
        }
        if log_enabled!(Level::Trace) {
            let n = N.load(Ordering::Relaxed);
            if n != 0 {
                trace!("max packet payload size: {n}");
            }
        }

        Ok(exit_code)
    }
}
