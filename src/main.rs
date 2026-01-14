use std::{
    fmt::Display,
    io::Write as _,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    ops::Not,
    process::ExitCode,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use env_logger::{Env, Target};
use log::{Level, error, info, warn};
use rayon::ThreadPool;
use socket2::{Domain, Protocol, Type};
use tokio::{
    net::UdpSocket,
    runtime::{Builder, Runtime},
    signal,
    sync::{OnceCell, Semaphore, SemaphorePermit, TryAcquireError, oneshot},
    task::JoinSet,
    time::{self},
};

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;
const UDP_PAYLOAD_MAX: usize = LINK_MTU_MAX - LINK_PAYLOAD_OFFSET;

const WORKER_THREADS: usize = 4;

const K: usize = 2usize.pow(10);
const M: usize = K.pow(2);

const RECV_BUF_SIZE: usize = 4 * M;
const SEND_BUF_SIZE: usize = M;

static RAW_SLICE: [u8; UDP_PAYLOAD_MAX] = [0; _];

static BUF_POOL: OnceCell<ArrayQueue<Box<[u8]>>> = OnceCell::const_new();
static THREAD_POOL: OnceCell<ThreadPool> = OnceCell::const_new();
static POOL_SEM: Semaphore = Semaphore::const_new(0);

static LOCAL_SOCKET: OnceCell<UdpSocket> = OnceCell::const_new();
static REMOTE_SOCKET: OnceCell<UdpSocket> = OnceCell::const_new();

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

mod local {
    use std::{
        sync::atomic::{AtomicBool, AtomicU64},
        time::Instant,
    };

    use tokio::sync::OnceCell;

    pub static CONNECTED: AtomicBool = AtomicBool::new(false);
    pub static LAST_SEEN: AtomicU64 = AtomicU64::new(0);
    pub static START: OnceCell<Instant> = OnceCell::const_new();
}

#[derive(Debug, Clone, Copy)]
enum Socket {
    Local,
    Remote,
}

impl Socket {
    fn get(&self) -> &UdpSocket {
        match *self {
            Socket::Local => LOCAL_SOCKET.get().unwrap(),
            Socket::Remote => REMOTE_SOCKET.get().unwrap(),
        }
    }
}

impl Not for Socket {
    type Output = Self;

    fn not(self) -> Self::Output {
        match self {
            Socket::Local => Socket::Remote,
            Socket::Remote => Socket::Local,
        }
    }
}

impl Display for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local => "local",
            Self::Remote => "remote",
        })
    }
}

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
    #[arg(flag = 'o', help = "client timeout in seconds")]
    time_out_f64_secs: Option<f64>,
    #[arg(flag = 't', help = "e.g. 0xFF")]
    token_hex_u8: Option<Box<str>>,
}

fn init_buf_pool(limit: usize, payload_max: usize) -> Result<()> {
    BUF_POOL.set(ArrayQueue::new(limit))?;
    (0..limit).for_each({
        let pool = BUF_POOL.get().unwrap();
        |_| pool.push(Box::from(&RAW_SLICE[..payload_max])).unwrap()
    });
    POOL_SEM.add_permits(limit);
    Ok(())
}

fn init_thread_pool(enable: bool) -> Result<()> {
    if !enable {
        return Ok(());
    }
    THREAD_POOL.set(
        rayon::ThreadPoolBuilder::new()
            .num_threads(
                thread::available_parallelism()?
                    .get()
                    .saturating_sub(WORKER_THREADS - 1) // sub 1 main thread
                    .max(2),
            )
            .stack_size(32 * K)
            .thread_name(|i| format!("xor-worker-{}", i))
            .build()?,
    )?;
    Ok(())
}

fn init_logger() {
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
        .init()
}

fn init_runtime(use_all_threads: bool) -> Result<Runtime> {
    Ok(Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            format!("tokio-runtime-{}", id)
        })
        .worker_threads(if use_all_threads {
            thread::available_parallelism()?.get()
        } else {
            WORKER_THREADS
        })
        .build()?)
}

struct Sockets {
    local: StdUdpSocket,
    remote: StdUdpSocket,
}

impl Sockets {
    fn new(listen_address: &str, remote_address: &str) -> Result<Self> {
        let listen_address: SocketAddr = listen_address.parse()?;
        let remote_address: SocketAddr = remote_address.parse()?;
        let glob: SocketAddr = "[::]:0".parse()?;

        let local_domain = match listen_address {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let local = socket2::Socket::new(local_domain, Type::DGRAM, Some(Protocol::UDP))?;
        let remote = socket2::Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;

        local.set_reuse_address(true)?;
        remote.set_reuse_address(true)?;

        if matches!(local_domain, Domain::IPV6) {
            local.set_only_v6(false)?;
        }
        remote.set_only_v6(false)?;

        local.set_nonblocking(true)?;
        remote.set_nonblocking(true)?;

        local.set_recv_buffer_size(RECV_BUF_SIZE)?;
        remote.set_recv_buffer_size(RECV_BUF_SIZE)?;

        local.set_send_buffer_size(SEND_BUF_SIZE)?;
        remote.set_send_buffer_size(SEND_BUF_SIZE)?;

        local.bind(&listen_address.into())?;
        remote.bind(&glob.into())?;

        remote.connect(&remote_address.into())?;

        let local = StdUdpSocket::from(local);
        let remote = StdUdpSocket::from(remote);

        Ok(Self { local, remote })
    }

    async fn convert(self) -> Result<()> {
        LOCAL_SOCKET.set(UdpSocket::from_std(self.local)?)?;
        REMOTE_SOCKET.set(UdpSocket::from_std(self.remote)?)?;
        Ok(())
    }
}

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
    let time_out = if let Some(time_out) = time_out_f64_secs {
        if time_out == 0. {
            Args::usage();
            return Ok(ExitCode::FAILURE);
        }
        time_out
    } else {
        2.
    };
    let token = if let Some(token) = token_hex_u8 {
        let s = token.trim_start_matches("0x");
        u8::from_str_radix(s, 16)?
    } else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };
    let limit = buffer_limit_usize.unwrap_or(768);
    let mtu = mtu_usize.unwrap_or(16384 + LINK_PAYLOAD_OFFSET);
    if mtu > LINK_MTU_MAX {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }
    let payload_max = mtu - LINK_PAYLOAD_OFFSET;

    let rt = init_runtime(token == 0)?;
    crate::local::START.set(Instant::now())?;
    init_buf_pool(limit, payload_max)?;
    init_thread_pool(token != 0)?;
    init_logger();
    let sockets = Sockets::new(&listen_address, &remote_address)?;

    let _main0 = Main0 {
        time_out,
        token,
        sockets,
    };
    rt.block_on(main0(_main0))
}

struct Main0 {
    time_out: f64,
    token: u8,
    sockets: Sockets,
}

async fn main0(main0: Main0) -> Result<ExitCode> {
    let Main0 {
        time_out,
        token,
        sockets,
    } = main0;
    sockets.convert().await?;

    let mut set = JoinSet::new();
    set.spawn(recv(token, Socket::Local));
    set.spawn(recv(token, Socket::Remote));

    info!("service started");

    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("shutting down");
            return Ok(ExitCode::SUCCESS);
        }
        _ = watch_dog(time_out) => {
            error!("the watch dog exited prematurely");
        },
        _ = set.join_next() => {
            error!("a network recv task exited prematurely");
        },
    }

    Ok(ExitCode::FAILURE)
}

async fn watch_dog(time_out: f64) {
    use crate::local::{CONNECTED, LAST_SEEN, START};
    let socket = LOCAL_SOCKET.get().unwrap();
    let start = START.get().unwrap();
    loop {
        let timeout_dur = Duration::from_secs_f64(time_out);
        time::sleep(Duration::from_secs_f64(time_out)).await;
        if CONNECTED.load(Ordering::Relaxed)
            && start
                .elapsed()
                .saturating_sub(Duration::from_millis(LAST_SEEN.load(Ordering::Relaxed)))
                > timeout_dur
        {
            if let Ok(addr) = socket.peer_addr() {
                info!("client timeout {addr}");
                if socket.connect("[::]:0").await.is_err() {
                    socket.connect("0.0.0.0:0").await.unwrap();
                };
            }
            CONNECTED.store(false, Ordering::Relaxed);
        }
    }
}

async fn send(
    mut buf: Box<[u8]>,
    n: usize,
    token: u8,
    current_socket: Socket,
    _permit: SemaphorePermit<'_>,
) {
    use crate::local::{LAST_SEEN, START};
    LAST_SEEN.store(
        Instant::now()
            .duration_since(*START.get().unwrap())
            .as_millis() as u64,
        Ordering::Relaxed,
    );
    let buf_pool = BUF_POOL.get().unwrap();
    let buf = if token != 0 {
        let (tx, rx) = oneshot::channel();
        THREAD_POOL.get().unwrap().spawn_fifo(move || {
            buf[..n].iter_mut().for_each(|b| *b ^= token);
            tx.send(buf).unwrap();
        });
        let Ok(buf) = rx.await else {
            SHUTDOWN.store(true, Ordering::Relaxed);
            return;
        };
        buf
    } else {
        buf
    };

    let socket = current_socket.get();
    if let Err(e) = socket.send(&buf[..n]).await {
        if SHUTDOWN
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            error!("{e}");
        }
        return;
    };

    if buf_pool.push(buf).is_err() {
        error!("failed to push buf back");
        SHUTDOWN.store(true, Ordering::Relaxed);
    }
}

#[inline(always)]
async fn connect(current_socket: Socket, addr: SocketAddr) {
    use crate::local::CONNECTED;

    if !matches!(current_socket, Socket::Local) {
        return;
    }

    if CONNECTED
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        if current_socket.get().connect(addr).await.is_ok() {
            info!("connected client {addr}");
        } else {
            CONNECTED.store(false, Ordering::Relaxed);
        }
    }
}

async fn recv(token: u8, current_socket: Socket) {
    let buf_pool = BUF_POOL.get().unwrap();
    let socket = current_socket.get();

    let try_push = |buf: Box<[u8]>| {
        if buf_pool.push(buf).is_err() {
            error!("failed to push buf back");
            SHUTDOWN.store(true, Ordering::Relaxed);
        }
    };

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return;
        }

        let Some(mut buf) = buf_pool.pop() else {
            let mut trashing = RAW_SLICE;
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
                SHUTDOWN.store(true, Ordering::Relaxed);
                return;
            }
        };

        tokio::spawn(connect(current_socket, addr));
        tokio::spawn(send(buf, n, token, !current_socket, _permit));
    }
}
