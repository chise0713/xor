use std::{
    fmt::Display,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    ops::{Deref, Not},
    sync::OnceLock,
};

use anyhow::Result;
use socket2::{Domain, Protocol, Type};
use tinystr::{TinyAsciiStr, tinystr};
use tokio::net::UdpSocket;

use crate::{M, NOT_INITED, ONCE, TINY_STR_STACK};

const RECV_BUF_SIZE: usize = 32 * M;
const SEND_BUF_SIZE: usize = RECV_BUF_SIZE;

static LOCAL_SOCKET: OnceLock<UdpSocket> = OnceLock::new();
static REMOTE_SOCKET: OnceLock<UdpSocket> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub enum Socket {
    Local,
    Remote,
}

impl Deref for Socket {
    type Target = UdpSocket;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        let fmt: TinyAsciiStr<TINY_STR_STACK> = tinystr!(32, "Socket: ").concat(NOT_INITED);
        let fmt = fmt.as_str();
        match *self {
            Socket::Local => LOCAL_SOCKET.get().expect(fmt),
            Socket::Remote => REMOTE_SOCKET.get().expect(fmt),
        }
    }
}

impl Not for Socket {
    type Output = Self;

    #[inline(always)]
    fn not(self) -> Self::Output {
        match self {
            Socket::Local => Socket::Remote,
            Socket::Remote => Socket::Local,
        }
    }
}

impl Display for Socket {
    #[inline(always)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local => "local",
            Self::Remote => "remote",
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
        let remote_address: SocketAddr = remote_address.parse()?;
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

        remote.connect(&remote_address.into())?;

        let local = StdUdpSocket::from(local);
        let remote = StdUdpSocket::from(remote);

        Ok(Self { local, remote })
    }

    pub fn convert(self) -> Result<()> {
        let fmt: TinyAsciiStr<TINY_STR_STACK> = tinystr!(32, "Sockets::convert(): ").concat(ONCE);
        let fmt = fmt.as_str();
        LOCAL_SOCKET
            .set(UdpSocket::from_std(self.local)?)
            .expect(fmt);
        REMOTE_SOCKET
            .set(UdpSocket::from_std(self.remote)?)
            .expect(fmt);
        Ok(())
    }
}
