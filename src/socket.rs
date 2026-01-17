use std::{
    fmt::Display,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    ops::{Deref, Not},
};

use anyhow::Result;
use socket2::{Domain, Protocol, Type};
use tokio::{net::UdpSocket, sync::OnceCell};

use crate::M;

const RECV_BUF_SIZE: usize = 4 * M;
const SEND_BUF_SIZE: usize = M;

static LOCAL_SOCKET: OnceCell<UdpSocket> = OnceCell::const_new();
static REMOTE_SOCKET: OnceCell<UdpSocket> = OnceCell::const_new();

#[derive(Debug, Clone, Copy)]
pub enum Socket {
    Local,
    Remote,
}

impl Deref for Socket {
    type Target = UdpSocket;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        match *self {
            Socket::Local => {
                let l = LOCAL_SOCKET.get();
                debug_assert!(l.is_some());
                unsafe { l.unwrap_unchecked() }
            }
            Socket::Remote => {
                let r = REMOTE_SOCKET.get();
                debug_assert!(r.is_some());
                unsafe { r.unwrap_unchecked() }
            }
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
        LOCAL_SOCKET.set(UdpSocket::from_std(self.local)?)?;
        REMOTE_SOCKET.set(UdpSocket::from_std(self.remote)?)?;
        Ok(())
    }
}
