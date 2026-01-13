use std::{
    io::Write as _,
    ops::Not,
    process::ExitCode,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use env_logger::{Env, Target};
use log::{Level, error, info};
use tokio::{
    net::UdpSocket,
    runtime::Builder,
    signal,
    sync::{OnceCell, Semaphore, SemaphorePermit},
    time::{self},
};

use crate::local::START;

const LINK_MTU_MAX: usize = 65535;
const UDP_HEADER: usize = 8;
const IPV6_HEADER: usize = 40;
const LINK_PAYLOAD_OFFSET: usize = UDP_HEADER + IPV6_HEADER;
const UDP_PAYLOAD_MAX: usize = LINK_MTU_MAX - LINK_PAYLOAD_OFFSET;

const WORKER_THREADS: usize = 4;

static RAW_SLICE: [u8; UDP_PAYLOAD_MAX] = [0; _];

static BUF_POOL: OnceCell<ArrayQueue<Box<[u8]>>> = OnceCell::const_new();
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
    #[arg(flag = 'o', help = "e.g. 2.5 is 2.5 secs")]
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

async fn init_sockets(listen_address: &str, remote_address: &str) -> Result<()> {
    LOCAL_SOCKET.set(UdpSocket::bind(listen_address).await?)?;
    REMOTE_SOCKET.set(UdpSocket::bind("[::]:0").await?)?;
    REMOTE_SOCKET.get().unwrap().connect(remote_address).await?;
    Ok(())
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
    let limit = buffer_limit_usize.unwrap_or(512);
    let mtu = mtu_usize.unwrap_or(1500);
    if mtu > LINK_MTU_MAX {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    }
    let payload_max = mtu - LINK_PAYLOAD_OFFSET;
    let rt = Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(
            thread::available_parallelism()?
                .get()
                .saturating_sub(WORKER_THREADS)
                .max(2),
        )
        .thread_keep_alive(Duration::from_secs(u64::MAX))
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            format!("xor-{}", id)
        })
        .worker_threads(WORKER_THREADS)
        .build()?;

    START.set(Instant::now())?;
    init_buf_pool(limit, payload_max)?;
    init_logger();

    rt.block_on(main0(Main0 {
        listen_address: &listen_address,
        remote_address: &remote_address,
        time_out,
        token,
    }))
}

struct Main0<'a> {
    listen_address: &'a str,
    remote_address: &'a str,
    time_out: f64,
    token: u8,
}

async fn main0(
    Main0 {
        listen_address,
        remote_address,
        time_out,
        token,
    }: Main0<'_>,
) -> Result<ExitCode> {
    init_sockets(listen_address, remote_address).await?;

    info!("service started");
    let mut set = tokio::task::JoinSet::new();
    set.spawn(recv(token, Socket::Local));
    set.spawn(recv(token, Socket::Remote));
    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("shutting down");
            return Ok(ExitCode::SUCCESS);
        }
        _ = watch_dog(time_out) => {
            error!("the watch dog exited prematurely");
        },
        _ = set.join_next() => {
            error!("a network recv exited prematurely");
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
    socket: Socket,
    _permit: SemaphorePermit<'_>,
) {
    let buf_pool = BUF_POOL.get().unwrap();
    let buf = tokio::task::spawn_blocking(move || {
        buf[..n].iter_mut().for_each(|b| *b ^= token);
        buf
    })
    .await
    .unwrap_or_default();
    if buf.is_empty() {
        SHUTDOWN.store(true, Ordering::Relaxed);
        return;
    }
    let socket = socket.get();
    if let Err(e) = socket.send(&buf[..n]).await {
        if SHUTDOWN
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            error!("{e}");
        }
        return;
    };
    buf_pool.push(buf).unwrap();
}

async fn recv(token: u8, current_socket: Socket) {
    use crate::local::{CONNECTED, LAST_SEEN};

    let buf_pool = BUF_POOL.get().unwrap();
    let socket = current_socket.get();

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return;
        }
        let _permit = POOL_SEM.acquire().await.unwrap();
        let mut buf = buf_pool.pop().unwrap();
        match socket.recv_from(buf.as_mut()).await {
            Ok((0, _)) => {
                error!("zero sized recv");
                break;
            }
            Ok((n, addr)) => {
                LAST_SEEN.store(
                    Instant::now()
                        .duration_since(*START.get().unwrap())
                        .as_millis() as u64,
                    Ordering::Relaxed,
                );
                if matches!(current_socket, Socket::Local)
                    && CONNECTED
                        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                        .is_ok()
                {
                    if socket.connect(addr).await.is_ok() {
                        info!("connected client {addr}");
                    } else {
                        CONNECTED.store(false, Ordering::Relaxed);
                    }
                }
                tokio::spawn(send(buf, n, token, !current_socket, _permit));
            }
            Err(e) => {
                error!("{e}");
                break;
            }
        }
    }
}
