#![cfg_attr(feature = "bench", feature(test))]

mod args;
mod buf_pool;
mod local;
mod logger;
mod shutdown;
mod socket;
mod xor;

use std::{
    io::ErrorKind,
    process::ExitCode,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use anyhow::Result;
use log::{error, info};
use log_limit::warn_limit_global;
use tokio::{
    runtime::Builder,
    signal,
    sync::{SemaphorePermit, TryAcquireError},
    task::{JoinSet, LocalSet},
    time,
};

use crate::{
    args::{Args, Parse as _, Usage as _},
    buf_pool::{AlignBox, BufPool, Semaphore},
    local::{ConnectCtx, LastSeen, LocalAddr, Started},
    logger::Logger,
    shutdown::Shutdown,
    socket::{Socket, Sockets},
    xor::xor,
};

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;

const K: usize = 2usize.pow(10);
const M: usize = K.pow(2);

const WARN_LIMIT_DUR: Duration = Duration::from_millis(250);

fn main() -> Result<ExitCode> {
    let Args {
        buffer_limit_usize,
        listen_address,
        mtu_usize,
        remote_address,
        time_out_f64_secs,
        token_hex_u8,
    } = match Args::parse() {
        Ok(v) => v,
        Err(e) => {
            return Ok(e);
        }
    };

    let Some(listen_address) = listen_address else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };
    let Some(remote_address) = remote_address else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };

    let time_out = time_out_f64_secs.unwrap_or(5.);
    if time_out == 0. {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }

    let Some(token) = token_hex_u8.and_then(|t| {
        t.strip_prefix("0x")
            .and_then(|s| u8::from_str_radix(s, 16).ok())
    }) else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };

    let mtu = mtu_usize.unwrap_or(16384 + LINK_PAYLOAD_OFFSET);
    if mtu > LINK_MTU_MAX {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }

    let payload_max = mtu - LINK_PAYLOAD_OFFSET;
    let limit = buffer_limit_usize.unwrap_or(K);

    let total_threads = thread::available_parallelism()?.get().max(1);
    const MAIN_THREAD: usize = 1;
    let worker_threads = total_threads.div_ceil(2).saturating_sub(MAIN_THREAD).max(1);
    let rt = Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(256 * K)
        .thread_name_fn(|| {
            static ID: AtomicUsize = AtomicUsize::new(0);
            let id = ID.fetch_add(1, Ordering::SeqCst);
            format!("xor-{}", id)
        })
        .worker_threads(total_threads)
        .build()?;

    let sockets = Sockets::new(&listen_address, &remote_address)?;
    coarsetime::Updater::new(50).start()?;
    BufPool::init(limit, payload_max)?;
    Logger::init();
    Started::now()?;

    rt.block_on(
        AsyncMain {
            time_out,
            token,
            sockets,
            worker_threads,
        }
        .enter(),
    )
}

struct AsyncMain {
    time_out: f64,
    token: u8,
    sockets: Sockets,
    worker_threads: usize,
}

impl AsyncMain {
    #[inline(always)]
    async fn enter(self) -> Result<ExitCode> {
        self.sockets.convert()?;

        let mut join_set = JoinSet::new();
        (0..self.worker_threads).for_each(|_| {
            join_set.spawn(recv(self.token, Socket::Remote));
            join_set.spawn(recv(self.token, Socket::Local));
        });
        let local_set = LocalSet::new();
        join_set.spawn_local_on(recv(self.token, Socket::Remote), &local_set);

        info!("service started");

        let mut exit_code = ExitCode::FAILURE;
        let mut net_fail = false;

        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutting down");
                Shutdown::request();
                exit_code = ExitCode::SUCCESS;
            },
            _ = watch_dog(self.time_out) => {
                error!("the watch dog exited prematurely");
            },
            _ = join_set.join_next() => {
                net_fail = true;
            },
            _ = local_set.run_until(recv(self.token, Socket::Local)) => {
                net_fail = true;
            }
        }
        join_set.abort_all();
        if net_fail {
            error!("a network recv task exited prematurely");
        }

        Ok(exit_code)
    }
}

#[inline(always)]
async fn watch_dog(time_out: f64) {
    let timeout_dur = Duration::from_secs_f64(time_out);
    let sleep_dur = Duration::from_secs_f64(time_out);

    loop {
        time::sleep(sleep_dur).await;
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

#[inline(always)]
fn try_push(buf: AlignBox) {
    if BufPool.push(buf).is_err() {
        error!("failed to push buf back");
        Shutdown::request();
    }
}

#[inline(always)]
fn send(
    mut buf: AlignBox,
    n: usize,
    token: u8,
    socket: Socket,
    is_local: bool,
    _p: SemaphorePermit<'_>,
) {
    if token != 0 {
        xor(buf.as_mut_ptr(), n, token);
    };

    let result = if is_local {
        let addr = LocalAddr::current();
        socket.try_send_to(&buf[..n], addr)
    } else {
        socket.try_send(&buf[..n])
    };
    match result {
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::WouldBlock => {
            warn_limit_global!(
                1,
                Duration::from_millis(250),
                "{socket} socket tx buffer is full, dropping packet"
            )
        }
        Err(e) => {
            if Shutdown::try_request() {
                error!("{e}");
            }
        }
    }

    try_push(buf);
}

async fn recv(token: u8, socket: Socket) {
    let is_local = matches!(socket, Socket::Local);
    while !Shutdown::requested() {
        let _p = match Semaphore.try_acquire() {
            Ok(_p) => _p,
            Err(TryAcquireError::Closed) => {
                break;
            }
            Err(TryAcquireError::NoPermits) => {
                warn_limit_global!(1, WARN_LIMIT_DUR, "semaphore backpressure");
                let _p = Semaphore.acquire().await;
                debug_assert!(_p.is_ok());
                unsafe { _p.unwrap_unchecked() }
            }
        };

        let Some(mut buf) = BufPool.pop() else {
            let mut trashing = [0u8];
            let _ = socket.recv_from(&mut trashing).await;
            warn_limit_global!(1, WARN_LIMIT_DUR, "buf_pool is empty, dropping packet");
            continue;
        };

        let (n, addr) = match socket.recv_from(buf.as_mut()).await {
            Ok(v) => v,
            Err(e) => {
                try_push(buf);
                error!("{e}");
                break;
            }
        };

        if is_local {
            let connected = ConnectCtx::is_connected();
            let local_addr = LocalAddr::current();
            if connected && addr == local_addr {
                LastSeen::now();
            } else if !connected {
                ConnectCtx::connect(addr);
            } else {
                warn_limit_global!(
                    1,
                    WARN_LIMIT_DUR,
                    "cached={local_addr}, current={addr}, dropping"
                );
                try_push(buf);
                continue;
            }
        }

        send(buf, n, token, !socket, !is_local, _p);
    }
}
