use std::{io::ErrorKind, net::SocketAddr, sync::atomic::Ordering};

use log::{Level, error, log_enabled};
use log_limit::warn_limit_global;

use crate::{
    N, WARN_LIMIT_DUR,
    buf_pool::{BufPool, LeasedBuf},
    local::{ConnectCtx, LastSeen, LocalAddr, NULL_SOCKET_ADDR},
    methods::{self, Method, MethodState},
    shutdown::Shutdown,
    socket::Socket,
};

pub struct RecvSend;

impl RecvSend {
    fn send(
        &self,
        mut buf: LeasedBuf,
        mut n: usize,
        socket: &Socket,
        method: &Method,
        cached_local: &SocketAddr,
    ) {
        // DnsPad is for: `local` -> self.pad() -> peer_padded
        // DnsUnPad is for: peer_padded -> self.unpad() -> `remote`
        n = match (method, socket) {
            (Method::Xor, _) => methods::xor(buf.as_mut_ptr(), n),
            (Method::DnsPad, Socket::Remote) | (Method::DnsUnPad, Socket::Local) => {
                methods::dns_pad(buf.as_mut_ptr(), n)
            }
            (Method::DnsPad, Socket::Local) | (Method::DnsUnPad, Socket::Remote) => {
                methods::dns_unpad(buf.as_mut_ptr(), n)
            }
        };

        if let Err(e) = match socket {
            Socket::Local => socket.try_send_to(&buf[..n], *cached_local),
            Socket::Remote => socket.try_send(&buf[..n]),
        } {
            match e.kind() {
                ErrorKind::WouldBlock => {
                    warn_limit_global!(1, WARN_LIMIT_DUR, "{socket} tx full, drop");
                }
                _ => {
                    if Shutdown::try_request() {
                        error!("{socket} socket: {e}");
                    }
                }
            }
        };
    }

    pub async fn recv(&self, socket: Socket) {
        let is_local = socket.is_local();
        let mut local_ver = 0;
        let mut cached_local = NULL_SOCKET_ADDR;
        let method = MethodState::current();
        while !Shutdown::requested() {
            let Some(mut buf) = BufPool::acquire().await else {
                break;
            };

            let (n, addr) = match socket.recv_from(buf.as_mut()).await {
                Ok(v) => v,
                Err(e) => {
                    error!("{e}");
                    break;
                }
            };
            if log_enabled!(Level::Trace) {
                N.fetch_max(n, Ordering::Relaxed);
            }

            if is_local && self.addtional_handle(&addr, &mut cached_local, &mut local_ver) {
                continue;
            };

            self.send(buf, n, &!socket, method, &cached_local);
        }
    }

    #[must_use]
    #[inline(never)]
    fn addtional_handle(
        &self,
        addr: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut u64,
    ) -> bool {
        if !ConnectCtx::is_connected() {
            ConnectCtx::connect(*addr);
            return false;
        }

        if LocalAddr::updated(local_ver) {
            *cached_local = LocalAddr::current();
        }

        if addr == cached_local {
            LastSeen::now();
            false
        } else {
            self.mismatch(cached_local, addr)
        }
    }

    #[cold]
    #[inline(never)]
    fn mismatch(&self, local: &SocketAddr, addr: &SocketAddr) -> bool {
        warn_limit_global!(1, WARN_LIMIT_DUR, "local={local}, current={addr}, dropping");
        true
    }
}
