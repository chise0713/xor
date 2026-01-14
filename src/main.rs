mod args;
mod buf_pool;
mod local;
mod logger;
mod shutdown;
mod socket;
mod thread_pool;
mod xor;

use std::{
    net::SocketAddr,
    ops::Mul as _,
    process::ExitCode,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use log::{error, info, warn};
use tokio::{
    runtime::Builder,
    signal,
    sync::{Semaphore, SemaphorePermit, TryAcquireError, oneshot},
    task::JoinSet,
    time,
};

use crate::{
    args::{Args, Parse as _, Usage as _},
    buf_pool::AlignBox,
    socket::{Socket, Sockets},
    xor::xor,
};

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;

const K: usize = 2usize.pow(10);
const M: usize = K.pow(2);

static POOL_SEM: Semaphore = Semaphore::const_new(0);

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

    let time_out = time_out_f64_secs.unwrap_or(2.);
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

    let limit = buffer_limit_usize.unwrap_or((K as f64).mul(1.5).round() as usize);

    let mtu = mtu_usize.unwrap_or(2usize.pow(14) + LINK_PAYLOAD_OFFSET);
    if mtu > LINK_MTU_MAX {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }
    let payload_max = mtu - LINK_PAYLOAD_OFFSET;

    buf_pool::init(limit, payload_max)?;

    let total_threads = thread::available_parallelism()?.get();

    let rt = Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(|| {
            static ID: AtomicUsize = AtomicUsize::new(0);
            let id = ID.fetch_add(1, Ordering::SeqCst);
            format!("tokio-runtime-{}", id)
        })
        .worker_threads(if token == 0 {
            total_threads
        } else {
            let worker_threads = total_threads.div_ceil(3).max(1);
            let compute_threads = total_threads.saturating_sub(worker_threads).max(1);
            thread_pool::init(compute_threads)?;
            worker_threads
        })
        .build()?;

    local::init_started_at();
    let sockets = Sockets::new(&listen_address, &remote_address)?;
    logger::init();

    rt.block_on(
        AsyncMain {
            time_out,
            token,
            sockets,
        }
        .enter(),
    )
}

struct AsyncMain {
    time_out: f64,
    token: u8,
    sockets: Sockets,
}

impl AsyncMain {
    async fn enter(self) -> Result<ExitCode> {
        self.sockets.convert()?;

        let mut join_set = JoinSet::new();
        join_set.spawn(recv(self.token, Socket::Local));
        join_set.spawn(recv(self.token, Socket::Remote));

        info!("service started");

        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutting down");
                return Ok(ExitCode::SUCCESS);
            }
            _ = watch_dog(self.time_out) => {
                error!("the watch dog exited prematurely");
            },
            _ = join_set.join_next() => {
                error!("a network recv task exited prematurely");
            },
        }

        Ok(ExitCode::FAILURE)
    }
}

async fn watch_dog(time_out: f64) {
    let socket = Socket::Local.get();
    let start = local::started_at();
    loop {
        let timeout_dur = Duration::from_secs_f64(time_out);
        time::sleep(Duration::from_secs_f64(time_out)).await;
        if !local::is_connected() {
            continue;
        }

        let dur = start.elapsed().saturating_sub(local::last_seen());
        if dur > timeout_dur {
            if let Ok(addr) = socket.peer_addr() {
                info!("client timeout {addr}");
                if socket.connect("[::]:0").await.is_err()
                    && socket.connect("0.0.0.0:0").await.is_err()
                {
                    error!("failed to disconnect {addr}");
                    shutdown::set(true);
                    return;
                };
            }
            local::set_connected(false);
        }
    }
}

async fn send(
    mut buf: AlignBox,
    n: usize,
    token: u8,
    current_socket: Socket,
    _permit: SemaphorePermit<'_>,
) {
    local::set_last_seen(Instant::now());

    let buf_pool = buf_pool::get();
    if token != 0 {
        let (tx, rx) = oneshot::channel();
        thread_pool::get().spawn_fifo(move || {
            xor(tx, buf, n, token);
        });

        let Ok(b) = rx.await else {
            shutdown::set(true);
            error!("sender dropped without sending");
            return;
        };
        buf = b;
    };

    let socket = current_socket.get();
    if let Err(e) = socket.send(&buf[..n]).await {
        if shutdown::cmp_exchange(false, true) {
            error!("{e}");
        }
        return;
    };

    if buf_pool.push(buf).is_err() {
        shutdown::set(true);
        error!("failed to push buf back");
    }
}

async fn connect(current_socket: Socket, addr: SocketAddr) {
    if local::cmp_exchange_connected(false, true) {
        if current_socket.get().connect(addr).await.is_ok() {
            info!("connected client {addr}");
        } else {
            local::set_connected(false);
        }
    }
}

async fn recv(token: u8, current_socket: Socket) {
    let buf_pool = buf_pool::get();
    let socket = current_socket.get();

    let try_push = |buf: AlignBox| {
        if buf_pool.push(buf).is_err() {
            shutdown::set(true);
            error!("failed to push buf back");
        }
    };

    loop {
        if shutdown::shutdown() {
            return;
        }

        let Some(mut buf) = buf_pool.pop() else {
            let mut trashing = [0u8];
            let _ = socket.recv_from(&mut trashing).await;
            warn!("buf_pool is empty, dropping packet");
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

        let _permit = match POOL_SEM.try_acquire() {
            Ok(p) => p,
            Err(e) => {
                if matches!(e, TryAcquireError::NoPermits) {
                    warn!("{current_socket} socket backpressure, dropping packet");
                    try_push(buf);
                    continue;
                }
                error!("failed to aquire permit: {}", e);
                shutdown::set(true);
                return;
            }
        };

        if matches!(current_socket, Socket::Local) {
            tokio::spawn(connect(current_socket, addr));
        }
        tokio::spawn(send(buf, n, token, !current_socket, _permit));
    }
}
