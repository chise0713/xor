use std::{io::ErrorKind, marker::PhantomData, net::SocketAddr, sync::atomic::Ordering};

use log::{Level, error, log_enabled, warn};
use log_limit::warn_limit_global;

use self::mode::{Mode, Modes};
use crate::{
    N, WARN_LIMIT_DUR,
    buf_pool::BufPool,
    local::{ConnectCtx, LastSeen, LocalAddr, LocalAddrState},
    methods::{Method, MethodState},
    shutdown::Shutdown,
    socket::Socket,
};

pub mod mode {
    use super::*;

    // idea stole from GPT
    pub enum Modes {
        Inbound,
        Outbound,
    }

    #[expect(private_bounds)]
    pub trait Mode: sealed::Sealed {
        const MODE: Modes;

        fn socket() -> Socket;

        #[inline]
        fn mode() -> Modes {
            Self::MODE
        }
    }

    pub struct Inbound;

    impl Mode for Inbound {
        const MODE: Modes = Modes::Inbound;

        fn socket() -> Socket {
            Socket::Inbound
        }
    }

    pub struct Outbound;

    impl Mode for Outbound {
        const MODE: Modes = Modes::Outbound;

        fn socket() -> Socket {
            Socket::Outbound
        }
    }

    mod sealed {
        pub(super) trait Sealed {}
        impl Sealed for super::Inbound {}
        impl Sealed for super::Outbound {}
    }
}

pub struct RecvSend<M: Mode> {
    _marker: PhantomData<M>,
}

impl<M: Mode> RecvSend<M> {
    fn send(
        &self,
        buf: &mut [u8],
        mut n: usize,
        socket: Socket,
        method: Method,
        cached_local: SocketAddr,
    ) {
        let from_outbound = matches!(M::mode(), Modes::Outbound);
        if let Err(e) = method.run(from_outbound, buf, &mut n) {
            warn!("{e}");
            return;
        };

        // Socket::Inbound.recv_from() -> process -> Socket::Outbound.send()
        // Socket::Outbound.recv() -> process -> Socket::Inbound.send_to()
        loop {
            if let Err(e) = if from_outbound {
                socket.try_send_to(&buf[..n], cached_local)
            } else {
                socket.try_send(&buf[..n])
            } {
                match e.kind() {
                    ErrorKind::WouldBlock => {
                        warn_limit_global!(1, WARN_LIMIT_DUR, "{socket} tx full, dropping");
                    }
                    ErrorKind::Interrupted => continue,
                    _ => {
                        if Shutdown::try_request() {
                            error!("{socket} socket: {e}");
                        }
                    }
                }
            }
            break;
        }
    }

    pub async fn recv(mode: M) {
        let _ = mode;
        let this = Self {
            _marker: PhantomData,
        };
        let socket = M::socket();

        let mut local_addr_state = LocalAddrState::default();

        let method = *MethodState::current();

        'outer: while !Shutdown::requested() {
            if let Err(e) = socket.readable().await {
                error!("{e}");
                break;
            };

            let mut buf = BufPool::acquire();

            loop {
                let (n, addr) = match socket.try_recv_from(buf.as_mut()) {
                    Ok(v) => v,
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock) => break,
                    Err(e) if matches!(e.kind(), ErrorKind::Interrupted) => continue,
                    Err(e) => {
                        error!("{e}");
                        break 'outer;
                    }
                };
                if log_enabled!(Level::Trace) {
                    N.fetch_max(n, Ordering::Relaxed);
                }

                if this.additional(addr, &mut local_addr_state) {
                    continue;
                }

                this.send(&mut buf, n, !socket, method, local_addr_state.addr());
            }
        }
    }

    #[must_use]
    fn additional(&self, addr: SocketAddr, local_addr_state: &mut LocalAddrState) -> bool {
        match M::mode() {
            Modes::Inbound => self.inbound_additional(addr, local_addr_state),
            Modes::Outbound => self.outbound_additional(local_addr_state),
        }
    }

    /// `&cached_local` will be sync in `RecvSend::<Outbound>`
    /// see `self.outbound_addtional()`
    #[must_use]
    #[inline(never)]
    fn inbound_additional(&self, addr: SocketAddr, local_addr_state: &mut LocalAddrState) -> bool {
        if !ConnectCtx::is_connected() {
            ConnectCtx::connect(addr);
            LocalAddr::check_and_update(local_addr_state);
            return false;
        }

        LocalAddr::check_and_update(local_addr_state);

        if addr == local_addr_state.addr() {
            LastSeen::now();
            return false;
        }

        self.mismatch(local_addr_state.addr(), addr)
    }

    /// `&cached_local` will be pass into send
    /// Socket::Inbound.try_send_to(buf, addr)
    #[inline]
    #[must_use]
    fn outbound_additional(&self, local_addr_state: &mut LocalAddrState) -> bool {
        LocalAddr::check_and_update(local_addr_state);
        false
    }

    #[cold]
    #[must_use]
    #[inline(never)]
    fn mismatch(&self, local: SocketAddr, addr: SocketAddr) -> bool {
        warn_limit_global!(1, WARN_LIMIT_DUR, "local={local}, current={addr}, dropping");
        true
    }
}
