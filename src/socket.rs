use std::{
    fmt::Display,
    io::{Error, ErrorKind},
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    ops::{Deref, Not},
    sync::OnceLock,
};

use anyhow::Result;
use socket2::{Domain, Protocol, Type};
use tokio::net::UdpSocket;

use crate::{INIT, M, ONCE, const_concat};

const RECV_BUF_SIZE: usize = 32 * M;
const SEND_BUF_SIZE: usize = RECV_BUF_SIZE;

static LOCAL_SOCKET: OnceLock<UdpSocket> = OnceLock::new();
static REMOTE_SOCKET: OnceLock<UdpSocket> = OnceLock::new();

#[repr(usize)]
#[derive(Debug, Clone, Copy)]
pub enum Socket {
    Inbound,
    Outbound,
}

impl Deref for Socket {
    type Target = UdpSocket;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        const_concat! {
            CTX = "Socket: " + INIT
        };
        match *self {
            Socket::Inbound => LOCAL_SOCKET.get().expect(&CTX),
            Socket::Outbound => REMOTE_SOCKET.get().expect(&CTX),
        }
    }
}

impl Not for Socket {
    type Output = Self;

    #[inline(always)]
    fn not(self) -> Self::Output {
        match self {
            Socket::Inbound => Socket::Outbound,
            Socket::Outbound => Socket::Inbound,
        }
    }
}

impl Display for Socket {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        })
    }
}

pub struct Sockets {
    local: StdUdpSocket,
    remote: StdUdpSocket,
}

impl Sockets {
    pub fn new(listen_address: &str, remote_address: &str) -> Result<Self> {
        let listen_address: SocketAddr = listen_address.parse()?;
        let glob: SocketAddr = "[::]:0".parse()?;

        let local_domain = match listen_address {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        };

        let local = socket2::Socket::new(local_domain, Type::DGRAM, Some(Protocol::UDP))?;
        let remote = socket2::Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;

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

        let local = StdUdpSocket::from(local);
        let remote = StdUdpSocket::from(remote);

        remote.connect(remote_address)?;

        Ok(Self { local, remote })
    }

    pub fn convert(self) -> Result<()> {
        let exist = |_| {
            const_concat! {
                CTX = "Socket::convert(): " + ONCE
            };
            Error::new(ErrorKind::AlreadyExists, CTX.as_str())
        };
        LOCAL_SOCKET
            .set(UdpSocket::from_std(self.local)?)
            .map_err(exist)?;
        REMOTE_SOCKET
            .set(UdpSocket::from_std(self.remote)?)
            .map_err(exist)?;
        Ok(())
    }
}
