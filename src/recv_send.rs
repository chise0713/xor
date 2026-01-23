use std::{io::ErrorKind, net::SocketAddr, sync::atomic::Ordering};

use log::{Level, error, log_enabled};
use log_limit::warn_limit_global;

use crate::{
    N, WARN_LIMIT_DUR,
    buf_pool::BufPool,
    local::{ConnectCtx, LastSeen, LocalAddr, NULL_SOCKET_ADDR},
    methods::{DnsPad, Method, MethodImpl as _, MethodState, Xor},
    shutdown::Shutdown,
    socket::Socket,
};

pub struct RecvSend;

impl RecvSend {
    #[inline(always)]
    fn send<const FROM_RECV: bool>(
        &self,
        buf: &mut [u8],
        mut n: usize,
        socket: &Socket,
        method: &Method,
        cached_local: &SocketAddr,
    ) {
        let is_local = !FROM_RECV;
        match method {
            m if m.is_symmetric() => Xor::apply(buf.as_mut_ptr(), &mut n),
            Method::DnsPad => {
                if is_local {
                    DnsPad::undo(buf.as_mut_ptr(), &mut n)
                } else {
                    DnsPad::apply(buf.as_mut_ptr(), &mut n)
                }
            }
            Method::DnsUnPad => {
                if is_local {
                    DnsPad::apply(buf.as_mut_ptr(), &mut n)
                } else {
                    DnsPad::undo(buf.as_mut_ptr(), &mut n)
                }
            }
            _ => unreachable!(),
        };

        if let Err(e) = if is_local {
            socket.try_send_to(&buf[..n], *cached_local)
        } else {
            socket.try_send(&buf[..n])
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

    pub async fn recv<const IS_LOCAL: bool>(&self) {
        let socket = if IS_LOCAL {
            Socket::Local
        } else {
            Socket::Remote
        };
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

            if self.addtional::<IS_LOCAL>(&addr, &mut cached_local, &mut local_ver) {
                continue;
            }

            self.send::<IS_LOCAL>(&mut buf, n, &!socket, method, &cached_local);
        }
    }

    #[must_use]
    #[inline(always)]
    fn addtional<const IS_LOCAL: bool>(
        &self,
        addr: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut u64,
    ) -> bool {
        if IS_LOCAL {
            self.local_addtional(addr, cached_local, local_ver)
        } else {
            self.remote_addtional(addr, cached_local, local_ver)
        }
    }

    #[must_use]
    #[inline(never)]
    fn local_addtional(
        &self,
        addr: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut u64,
    ) -> bool {
        if !ConnectCtx::is_connected() {
            ConnectCtx::connect(*addr);
            *cached_local = *addr;
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

    #[must_use]
    #[inline(never)]
    fn remote_addtional(
        &self,
        _: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut u64,
    ) -> bool {
        if LocalAddr::updated(local_ver) {
            *cached_local = LocalAddr::current();
        }
        false
    }

    #[cold]
    #[must_use]
    #[inline(never)]
    fn mismatch(&self, local: &SocketAddr, addr: &SocketAddr) -> bool {
        warn_limit_global!(1, WARN_LIMIT_DUR, "local={local}, current={addr}, dropping");
        true
    }
}
