use std::{io::ErrorKind, net::SocketAddr, sync::atomic::Ordering};

use log::{Level, error, log_enabled, warn};
use log_limit::warn_limit_global;

use self::mode::Mode;
use crate::{
    N, WARN_LIMIT_DUR,
    buf_pool::BufPool,
    local::{ConnectCtx, LastSeen, LocalAddr, NULL_SOCKET_ADDR},
    methods::{
        DnsPad, Method, MethodImpl as _, MethodState, Xor,
        dns_pad::{self, DNS_QUERY_LEN},
    },
    shutdown::Shutdown,
    socket::Socket,
};

pub mod mode {
    // idea stole from GPT
    pub trait Mode: private::Sealed {
        const IS_LOCAL: bool;
    }

    pub struct Local;
    pub struct Remote;

    impl Mode for Local {
        const IS_LOCAL: bool = true;
    }

    impl Mode for Remote {
        const IS_LOCAL: bool = false;
    }

    mod private {
        pub trait Sealed {}
        impl Sealed for super::Local {}
        impl Sealed for super::Remote {}
    }
}

pub struct RecvSend;

impl RecvSend {
    #[inline(always)]
    fn send<M: Mode>(
        &self,
        buf: &mut [u8],
        mut n: usize,
        socket: Socket,
        method: Method,
        cached_local: &SocketAddr,
    ) {
        let is_local = !M::IS_LOCAL;
        match method {
            Method::DnsPad | Method::DnsUnPad => {
                // do apply at `Socket::Remote.send()` to peer
                // other vise undo peer apply and `Socket::Local.send_to()`
                let do_apply = matches!(method, Method::DnsPad) ^ is_local;
                if do_apply {
                    if !dns_pad::runtime_apply_check(buf.len(), n) {
                        warn!("dns pad overflow: cap={}, n={n}", buf.len());
                        return;
                    }
                    unsafe { DnsPad::apply(buf.as_mut_ptr(), &mut n) };
                } else {
                    if !dns_pad::runtime_undo_check(n) {
                        warn!("dns unpad underflow: {n} < {DNS_QUERY_LEN}");
                        return;
                    }
                    unsafe { DnsPad::undo(buf.as_mut_ptr(), &mut n) };
                }
            }

            Method::Xor => {
                // xor is symmetrical
                unsafe { Xor::apply(buf.as_mut_ptr(), &mut n) };
            }
        }

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

    pub async fn recv<M: Mode>(&self) {
        let socket = if M::IS_LOCAL {
            Socket::Local
        } else {
            Socket::Remote
        };
        let mut local_ver = 0;
        let mut cached_local = NULL_SOCKET_ADDR;
        let method = *MethodState::current();
        while !Shutdown::requested() {
            let Some(mut buf) = BufPool::acquire().await else {
                error!("semaphore closed");
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

            if self.addtional::<M>(&addr, &mut cached_local, &mut local_ver) {
                continue;
            }

            self.send::<M>(&mut buf, n, !socket, method, &cached_local);
        }
    }

    #[must_use]
    #[inline(always)]
    fn addtional<M: Mode>(
        &self,
        addr: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut usize,
    ) -> bool {
        if M::IS_LOCAL {
            self.local_addtional(addr, cached_local, local_ver)
        } else {
            self.remote_addtional(addr, cached_local, local_ver)
        }
    }

    // `&cached_local` will be sync in `RecvSend::<Remote>`
    // see `self.remote_addtional()`
    #[must_use]
    #[inline(never)]
    fn local_addtional(
        &self,
        addr: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut usize,
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
            return false;
        }

        self.mismatch(cached_local, addr)
    }

    // `&cached_local` will be pass into send
    // Socket::Local.try_send_to(buf, addr)
    #[must_use]
    #[inline(never)]
    fn remote_addtional(
        &self,
        _: &SocketAddr,
        cached_local: &mut SocketAddr,
        local_ver: &mut usize,
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
