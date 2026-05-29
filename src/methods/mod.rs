macro_rules! use_impl_struct {
    ($($mod_name:ident),*) => {
        $(
            ::paste::paste! {
                pub mod $mod_name;
                pub use $mod_name::[< $mod_name:camel >];
            }
        )*
    };
}
use_impl_struct!(xor, dns_pad);

use std::{
    fmt::Display,
    io::{Error, ErrorKind},
    mem,
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;

use crate::{INIT, ONCE, const_concat, uninit_panic};

pub trait MethodApply {
    fn apply(buf: &mut [u8], n: &mut usize) -> Result<()>;
}

pub trait MethodUndo {
    fn undo(buf: &mut [u8], n: &mut usize) -> Result<()>;
}

#[repr(usize)]
#[derive(Default, Clone, Copy)]
pub enum Method {
    #[default]
    Xor,
    DnsPad,
    DnsUnPad,
}

impl Method {
    fn as_str(&self) -> &'static str {
        match self {
            Method::Xor => "xor",
            Method::DnsPad => "dnspad",
            Method::DnsUnPad => "dnsunpad",
        }
    }

    pub fn run(self, from_outbound: bool, buf: &mut [u8], n: &mut usize) -> Result<()> {
        match self {
            Method::DnsPad | Method::DnsUnPad => {
                let apply = matches!(self, Method::DnsPad) ^ from_outbound;
                if apply {
                    DnsPad::apply(buf, n)
                } else {
                    DnsPad::undo(buf, n)
                }
            }

            Method::Xor => Xor::apply(buf, n),
        }
    }
}

impl FromStr for Method {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "xor" => Ok(Method::Xor),
            "dnspad" => Ok(Method::DnsPad),
            "dnsunpad" => Ok(Method::DnsUnPad),
            _ => Err(()),
        }
    }
}

impl Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

static CURRENT_METHOD: AtomicUsize = AtomicUsize::new(MethodState::SENTINEL);

pub struct MethodState;

impl MethodState {
    const SENTINEL: usize = usize::MAX;

    #[cold]
    #[inline(never)]
    pub fn init(method: Method) -> Result<()> {
        let exist = |_| {
            const_concat! {
                CTX = "MethodState::set()" + ONCE
            }
            Error::new(ErrorKind::AlreadyExists, CTX.as_str())
        };

        CURRENT_METHOD
            .compare_exchange(
                Self::SENTINEL,
                method as usize,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .map_err(exist)?;
        Ok(())
    }

    #[inline]
    pub fn current() -> Method {
        const_concat! {
            CTX = "MethodState::current()" + INIT
        }
        let method = CURRENT_METHOD.load(Ordering::Relaxed);

        if method == Self::SENTINEL {
            uninit_panic(CTX.as_str());
        }

        // SAFETY:
        // - CURRENT_METHOD only stores `Method as usize`
        // - SENTINEL is filtered before transmute
        unsafe { mem::transmute(method) }
    }
}
