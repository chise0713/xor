use std::{io::Write as _, process::ExitCode, sync::Arc, thread};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use env_logger::{Env, Target};
use log::{Level, debug, error, info, warn};
use tokio::{
    net::UdpSocket,
    signal,
    sync::mpsc::{self, error::TrySendError},
};

const K: usize = 1024;

#[derive(supershorty::Args, Debug)]
#[args(name = "xor")]
struct Args {
    #[arg(flag = 'b', help = "for limiting total pre-allocated buffer")]
    buffer_limit_usize: Option<usize>,
    #[arg(flag = 'l', help = "listen address")]
    listen_address: Option<Box<str>>,
    #[arg(flag = 'm', help = "for payload mtu")]
    mtu_usize: Option<usize>,
    #[arg(flag = 'r', help = "remote address")]
    remote_address: Option<Box<str>>,
    #[arg(flag = 't', help = "e.g. 0xFF")]
    token_hex_u8: Option<Box<str>>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<ExitCode> {
    let Args {
        listen_address,
        remote_address,
        token_hex_u8,
        buffer_limit_usize,
        mtu_usize,
    } = match Args::parse() {
        Ok(v) => v,
        Err(e) => {
            return Ok(e);
        }
    };

    let Some(listen) = listen_address else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };
    let Some(remote) = remote_address else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };
    let token = if let Some(token) = token_hex_u8 {
        let s = token.trim_start_matches("0x");
        u8::from_str_radix(s, 16)?
    } else {
        Args::usage();
        return Ok(ExitCode::FAILURE);
    };
    let limit = buffer_limit_usize.unwrap_or(512);
    let mtu = mtu_usize.unwrap_or(1452);

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

    let listen = UdpSocket::bind(listen.as_ref()).await?;
    let send_socket = UdpSocket::bind("[::]:0").await?;
    send_socket.connect(remote.as_ref()).await?;
    let (tx, mut rx) = mpsc::channel(limit);
    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(thread::available_parallelism()?.get())
            .stack_size(256 * K)
            .thread_name(|i| format!("xor-worker-{}", i))
            .build()?,
    );
    let buf_pool = Arc::new(ArrayQueue::new(limit));
    (0..limit).for_each(|_| {
        let stack = [0u8; 65527];
        buf_pool.push(Box::from(&stack[..mtu])).unwrap()
    });
    let send_handle = tokio::spawn({
        let reclaim = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .stack_size(16 * K)
                .thread_name(|_| "reclaim-worker".to_string())
                .build()?,
        );
        let buf_pool = buf_pool.clone();
        async move {
            loop {
                let buf_pool = buf_pool.clone();
                if let Some((data, n)) = rx.recv().await {
                    let data: Box<[u8]> = data;
                    match send_socket.send(&data[..n]).await {
                        Ok(n) => debug!("send {n} packets to {remote}"),
                        Err(e) => {
                            error!("{e}");
                            break;
                        }
                    };
                    reclaim.spawn_fifo(move || buf_pool.push(data).unwrap());
                } else {
                    break;
                }
            }
        }
    });
    info!("service started");
    let recv_handle = tokio::spawn({
        let buf_pool = buf_pool.clone();
        async move {
            loop {
                let mut buf = match buf_pool.pop() {
                    Some(b) => b,
                    None => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                };
                let tx = tx.clone();
                match listen.recv(buf.as_mut()).await {
                    Ok(0) => {
                        error!("zero sized recv");
                        break;
                    }
                    Ok(n) => {
                        pool.spawn_fifo(move || {
                            buf[..n].iter_mut().for_each(|b| *b ^= token);
                            if let Err(TrySendError::Full(buf)) = tx.try_send((buf, n)) {
                                warn!("blocked");
                                _ = tx.blocking_send(buf);
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
    });
    let recv_abt = recv_handle.abort_handle();
    let send_abt = send_handle.abort_handle();
    tokio::select! {
        r = signal::ctrl_c() => {
            r.unwrap();
            recv_abt.abort();
            send_abt.abort();
            info!("shutting down");
        }
        _ = recv_handle => {},
        _ = send_handle => {},
    }
    Ok(ExitCode::SUCCESS)
}
