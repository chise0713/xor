use std::{io::Write as _, process::ExitCode, sync::Arc, thread, time::Duration};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use env_logger::{Env, Target};
use log::{Level, debug, error, info, warn};
use rayon::ThreadPool;
use tokio::{
    net::UdpSocket,
    signal,
    sync::{
        OnceCell, RwLock,
        mpsc::{self, Receiver, Sender, error::TrySendError},
    },
    time::{self, Instant},
};

const K: usize = 1024;

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;
const UDP_PAYLOAD_MAX: usize = LINK_MTU_MAX - LINK_PAYLOAD_OFFSET;

static RAW_SLICE: [u8; UDP_PAYLOAD_MAX] = [0; _];

static THREAD_POOL: OnceCell<ThreadPool> = OnceCell::const_new();
static BUF_POOL: OnceCell<ArrayQueue<Box<[u8]>>> = OnceCell::const_new();

static mut TOKEN: u8 = 0;

#[derive(supershorty::Args, Debug)]
#[args(name = "xor")]
struct Args {
    #[arg(flag = 'b', help = "for total pre-allocated buffer")]
    buffer_limit_usize: Option<usize>,
    #[arg(flag = 'l', help = "listen address")]
    listen_address: Option<Box<str>>,
    #[arg(flag = 'm', help = "for link mtu")]
    mtu_usize: Option<usize>,
    #[arg(flag = 'r', help = "remote address")]
    remote_address: Option<Box<str>>,
    #[arg(flag = 't', help = "e.g. 0xFF")]
    token_hex_u8: Option<Box<str>>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<ExitCode> {
    let Args {
        buffer_limit_usize,
        listen_address,
        mtu_usize,
        remote_address,
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
    if let Some(token) = token_hex_u8 {
        let s = token.trim_start_matches("0x");
        unsafe { TOKEN = u8::from_str_radix(s, 16)? };
    } else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };
    let limit = buffer_limit_usize.unwrap_or(512);
    let mtu = mtu_usize.unwrap_or(1500);
    if mtu > LINK_MTU_MAX {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }
    let payload_max = mtu - LINK_PAYLOAD_OFFSET;

    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .target(Target::Stdout)
        .format(move |buf, record| {
            let level_str = match record.level() {
                Level::Trace => "\x1B[1;35mTRACE\x1B[0m",
                Level::Debug => "\x1B[1;30mDEBUG\x1B[0m",
                Level::Info => "\x1B[1;36mINFO\x1B[0m",
                Level::Warn => "\x1B[1;93mWARN\x1B[0m",
                Level::Error => "\x1B[1;31mERROR\x1B[0m",
            };
            writeln!(
                buf,
                "[{} {}:{} {}]: {}",
                buf.timestamp_millis(),
                record.file().unwrap(),
                record.line().unwrap(),
                level_str,
                record.args()
            )
        })
        .init();

    let local = Arc::new(UdpSocket::bind(listen_address.as_ref()).await?);
    let remote = Arc::new(UdpSocket::bind("[::]:0").await?);
    remote.connect(remote_address.as_ref()).await?;
    let (tx_to_remote, rx_from_local) = mpsc::channel(limit);
    let (tx_to_local, rx_from_remote) = mpsc::channel(limit);
    THREAD_POOL.set(
        rayon::ThreadPoolBuilder::new()
            .num_threads(thread::available_parallelism()?.get() - 4)
            .stack_size(64 * K)
            .thread_name(|i| format!("xor-worker-{}", i))
            .build()?,
    )?;
    BUF_POOL.set(ArrayQueue::new(limit))?;
    (0..limit).for_each({
        let pool = BUF_POOL.get().unwrap();
        |_| pool.push(Box::from(&RAW_SLICE[..payload_max])).unwrap()
    });

    info!("service started");
    let remote_send = tokio::spawn(send(rx_from_local, remote.clone()));
    let remote_recv = tokio::spawn(recv(tx_to_local, remote));
    let local_send = tokio::spawn(send(rx_from_remote, local.clone()));
    let local_recv = tokio::spawn(recv(tx_to_remote, local));

    let remote_send_abt = remote_send.abort_handle();
    let remote_recv_abt = remote_recv.abort_handle();
    let local_send_abt = local_send.abort_handle();
    let local_recv_abt = local_recv.abort_handle();
    tokio::select! {
        r = signal::ctrl_c() => {
            r.unwrap();
            remote_send_abt.abort();
            remote_recv_abt.abort();
            local_send_abt.abort();
            local_recv_abt.abort();
            info!("shutting down");
            Ok(ExitCode::SUCCESS)
        }
        _ = remote_send => Ok(ExitCode::FAILURE),
        _ = remote_recv => Ok(ExitCode::FAILURE),
        _ = local_send => Ok(ExitCode::FAILURE),
        _ = local_recv => Ok(ExitCode::FAILURE),
    }
}

async fn send(mut rx: Receiver<(Box<[u8]>, usize)>, socket: Arc<UdpSocket>) {
    let thread_pool = THREAD_POOL.get().unwrap();
    let buf_pool = BUF_POOL.get().unwrap();
    while let Some((buf, n)) = rx.recv().await {
        let buf: Box<[u8]> = buf;
        if let Err(e) = socket.send(&buf[..n]).await {
            error!("{e}");
            break;
        };
        thread_pool.spawn(move || buf_pool.push(buf).unwrap());
    }
}

async fn recv(tx: Sender<(Box<[u8]>, usize)>, socket: Arc<UdpSocket>) {
    let buf_pool = BUF_POOL.get().unwrap();
    let thread_pool = THREAD_POOL.get().unwrap();
    let token = unsafe { TOKEN };
    let is_listen = Arc::new(RwLock::new(false));
    let last_seen = Arc::new(RwLock::new(Instant::now()));
    tokio::spawn({
        let socket = socket.clone();
        let last_seen = last_seen.clone();
        let is_listen = is_listen.clone();
        async move {
            loop {
                time::sleep(Duration::from_secs(2)).await;
                if *is_listen.read().await
                    && let Ok(addr) = socket.peer_addr()
                    && last_seen.read().await.elapsed() > Duration::from_secs(2)
                {
                    info!("client timeout {addr}");
                    if socket.connect("[::]:0").await.is_err() {
                        socket.connect("0.0.0.0:0").await.unwrap();
                    };
                    debug!("removed local connect address: {addr}");
                    *is_listen.write().await = false;
                }
            }
        }
    });
    loop {
        let mut buf = match buf_pool.pop() {
            Some(b) => b,
            None => {
                tokio::task::yield_now().await;
                continue;
            }
        };
        let tx = tx.clone();
        match socket.recv_from(buf.as_mut()).await {
            Ok((0, _)) => {
                error!("zero sized recv");
                break;
            }
            Ok((n, addr)) => {
                if socket.peer_addr().is_err() {
                    *is_listen.write().await = true;
                    socket.connect(addr).await.unwrap();
                    *last_seen.write().await = Instant::now();
                } else if *is_listen.read().await {
                    socket.connect(addr).await.unwrap();
                    *last_seen.write().await = Instant::now();
                }
                thread_pool.spawn_fifo(move || {
                    if token != 0 {
                        buf[..n].iter_mut().for_each(|b| *b ^= token);
                    }
                    if let Err(TrySendError::Full((buf, n))) = tx.try_send((buf, n)) {
                        warn!("blocked");
                        _ = tx.blocking_send((buf, n));
                    };
                });
            }
            Err(e) => {
                error!("{e}");
                break;
            }
        }
    }
}
