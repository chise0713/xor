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
    time::Duration,
};

use anyhow::Result;
use log::{error, info, warn};
use tokio::{
    runtime::Builder,
    signal,
    sync::{SemaphorePermit, TryAcquireError, oneshot},
    task::{self, JoinSet},
    time,
};

use crate::{
    args::{Args, Parse as _, Usage as _},
    buf_pool::{AlignBox, BufPool, Semaphore},
    local::{ConnectCtx, LastSeen, LocalAddr, Started},
    logger::Logger,
    shutdown::Shutdown,
    socket::{Socket, Sockets},
    thread_pool::ThreadPool,
    xor::xor,
};

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;

const K: usize = 2usize.pow(10);
const M: usize = K.pow(2);

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

    let mtu = mtu_usize.unwrap_or(2usize.pow(14) + LINK_PAYLOAD_OFFSET);
    if mtu > LINK_MTU_MAX {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }

    let payload_max = mtu - LINK_PAYLOAD_OFFSET;
    let limit = buffer_limit_usize.unwrap_or((K as f64).mul(1.5).round() as usize);

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
            ThreadPool::init(compute_threads)?;
            worker_threads
        })
        .build()?;

    BufPool::init(limit, payload_max)?;
    Started::init()?;
    let sockets = Sockets::new(&listen_address, &remote_address)?;
    Logger::init();

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
    #[inline(always)]
    async fn enter(self) -> Result<ExitCode> {
        self.sockets.convert()?;

        let mut join_set = JoinSet::new();
        join_set.spawn(recv(self.token, Socket::Local));
        join_set.spawn(recv(self.token, Socket::Remote));

        info!("service started");

        let mut exit_code = ExitCode::FAILURE;

        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("shutting down");
                exit_code = ExitCode::SUCCESS;
            }
            _ = watch_dog(self.time_out) => {
                error!("the watch dog exited prematurely");
            },
            _ = join_set.join_next() => {
                error!("a network recv task exited prematurely");
            },
        }
        join_set.shutdown().await;

        Ok(exit_code)
    }
}

async fn watch_dog(time_out: f64) {
    let socket = Socket::Local;
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
        ConnectCtx::disconnect();

        if let Ok(addr) = socket.peer_addr() {
            LocalAddr::clear().await;
            info!("client timeout {addr}");
        }
    }
}

async fn send(buf: AlignBox, n: usize, token: u8, socket: Socket, _permit: SemaphorePermit<'_>) {
    task::spawn_blocking(LastSeen::now);

    let (tx, rx) = oneshot::channel();

    if token == 0 {
        _ = tx.send(buf);
    } else {
        ThreadPool.spawn_fifo(move || {
            xor(tx, buf, n, token);
        });
    };

    let Ok(buf) = rx.await else {
        error!("sender dropped without sending");
        Shutdown::request();
        return;
    };

    if matches!(socket, Socket::Local) {
        if let Err(e) = socket.send_to(&buf[..n], LocalAddr::current().await).await
            && Shutdown::try_request()
        {
            error!("{socket} socket: {e}");
        };
    } else {
        if let Err(e) = socket.send(&buf[..n]).await
            && Shutdown::try_request()
        {
            error!("{socket} socket: {e}");
        };
    }

    if BufPool.push(buf).is_err() {
        error!("failed to push buf back");
        Shutdown::request();
    }
}

async fn connect(socket: Socket, addr: SocketAddr) {
    if ConnectCtx::try_connect() && socket.connect(addr).await.is_ok() {
        LocalAddr::set(addr).await;
        info!("set client addr to {addr}");
    }
}

async fn recv(token: u8, socket: Socket) {
    let try_push = |buf: AlignBox| {
        if BufPool.push(buf).is_err() {
            error!("failed to push buf back");
            Shutdown::request();
        }
    };

    while !Shutdown::requested() {
        let Some(mut buf) = BufPool.pop() else {
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

        let _permit = match Semaphore.try_acquire() {
            Ok(p) => p,
            Err(e) => {
                if matches!(e, TryAcquireError::NoPermits) {
                    warn!("{socket} socket backpressure, dropping packet");
                    try_push(buf);
                    continue;
                }
                error!("failed to aquire permit: {}", e);
                Shutdown::request();
                return;
            }
        };

        if matches!(socket, Socket::Local) {
            tokio::spawn(connect(socket, addr));
        }
        tokio::spawn(send(buf, n, token, !socket, _permit));
    }
}
