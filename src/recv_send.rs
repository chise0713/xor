use std::{io::ErrorKind, net::SocketAddr, sync::atomic::Ordering};

use log::{Level, error, log_enabled, warn};
use log_limit::warn_limit_global;

use self::mode::{Mode, Modes};
use crate::{
    N, WARN_LIMIT_DUR,
    buf_pool::BufPool,
    local::{ConnectCtx, LastSeen, LocalAddr, NULL_SOCKET_ADDR},
    methods::{DnsPad, Method, MethodApply as _, MethodState, MethodUndo as _, Xor},
    shutdown::Shutdown,
    socket::Socket,
};

pub mod mode {
    // idea stole from GPT
    #[repr(u8)]
    pub enum Modes {
        Inbound = 0,
        Outbound = 1,
    }

    #[expect(private_bounds)]
    pub trait Mode: sealed::Sealed {
        const MODE: u8;

        #[inline(always)]
        fn mode() -> Modes {
            match Self::MODE {
                0 => Modes::Inbound,
                1 => Modes::Outbound,
                _ => unimplemented!(),
            }
        }
    }

    pub struct Inbound;

    impl Mode for Inbound {
        const MODE: u8 = 0;
    }

    pub struct Outbound;

    impl Mode for Outbound {
        const MODE: u8 = 1;
    }

    mod sealed {
        pub(super) trait Sealed {}
        impl Sealed for super::Inbound {}
        impl Sealed for super::Outbound {}
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
        cached_local: SocketAddr,
    ) {
        let from_outbound = matches!(M::mode(), Modes::Outbound);
        match method {
            Method::DnsPad | Method::DnsUnPad => {
                // do apply at `Socket::Outbound.send()` to peer
                // other vise undo peer apply and `Socket::Inbound.send_to()`
                if matches!(method, Method::DnsPad) ^ from_outbound {
                    if let Err(e) = DnsPad::apply(buf, &mut n) {
                        warn!("{e}");
                        return;
                    }
                } else {
                    if let Err(e) = DnsPad::undo(buf, &mut n) {
                        warn!("{e}");
                        return;
                    }
                }
            }

            Method::Xor => {
                // xor is symmetrical
                if let Err(e) = Xor::apply(buf, &mut n) {
                    warn!("{e}");
                    return;
                }
            }
        }

        // Socket::Inbound.recv_from() -> process -> Socket::Outbound.send()
        // Socket::Outbound.recv() -> process -> Socket::Inbound.send_to()
        if let Err(e) = if from_outbound {
            socket.try_send_to(&buf[..n], cached_local)
        } else {
            socket.try_send(&buf[..n])
        } {
            match e.kind() {
                ErrorKind::WouldBlock => {
                    warn_limit_global!(1, WARN_LIMIT_DUR, "{socket} tx full, dropping");
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
        let socket = match M::mode() {
            Modes::Inbound => Socket::Inbound,
            Modes::Outbound => Socket::Outbound,
        };
        let mut cached_ver = 0;
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

            if self.additional::<M>(addr, &mut cached_local, &mut cached_ver) {
                continue;
            }

            self.send::<M>(&mut buf, n, !socket, method, cached_local);
        }
    }

    #[must_use]
    fn additional<M: Mode>(
        &self,
        addr: SocketAddr,
        cached_local: &mut SocketAddr,
        cached_ver: &mut usize,
    ) -> bool {
        if matches!(M::mode(), Modes::Inbound) {
            self.inbound_additional(addr, cached_local, cached_ver)
        } else {
            self.outbound_additional(cached_local, cached_ver)
        }
    }

    /// `&cached_local` will be sync in `RecvSend::<Outbound>`
    /// see `self.outbound_addtional()`
    #[must_use]
    #[inline(never)]
    fn inbound_additional(
        &self,
        addr: SocketAddr,
        cached_local: &mut SocketAddr,
        cached_ver: &mut usize,
    ) -> bool {
        if !ConnectCtx::is_connected() {
            ConnectCtx::connect(addr);
            *cached_local = addr;
            *cached_ver = LocalAddr::version();
            return false;
        }

        if LocalAddr::check_and_update(cached_ver) {
            *cached_local = LocalAddr::current();
        }

        if addr == *cached_local {
            LastSeen::now();
            return false;
        }

        self.mismatch(cached_local, addr)
    }

    /// `&cached_local` will be pass into send
    /// Socket::Inbound.try_send_to(buf, addr)
    #[must_use]
    fn outbound_additional(&self, cached_local: &mut SocketAddr, cached_ver: &mut usize) -> bool {
        if LocalAddr::check_and_update(cached_ver) {
            *cached_local = LocalAddr::current();
        }
        false
    }

    #[cold]
    #[must_use]
    #[inline(never)]
    fn mismatch(&self, local: &SocketAddr, addr: SocketAddr) -> bool {
        warn_limit_global!(1, WARN_LIMIT_DUR, "local={local}, current={addr}, dropping");
        true
    }
}
