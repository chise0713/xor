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
    str::FromStr,
    sync::OnceLock,
};

use anyhow::{Result, bail};

use crate::{INIT, ONCE, const_concat};

pub trait MethodImpl {
    unsafe fn apply(ptr: *mut u8, n: &mut usize);
    #[inline(always)]
    unsafe fn undo(ptr: *mut u8, n: &mut usize) {
        unsafe { Self::apply(ptr, n) }
    }
}

#[inline(always)]
fn align_check(ptr: usize) {
    use crate::buf_pool::SIMD_WIDTH;

    if !ptr.is_multiple_of(SIMD_WIDTH) {
        unreachable!("buf must be {}B aligned", SIMD_WIDTH)
    }
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
}

impl FromStr for Method {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "xor" => Ok(Method::Xor),
            "dnspad" => Ok(Method::DnsPad),
            "dnsunpad" => Ok(Method::DnsUnPad),
            _ => bail!("unknown method: {s}"),
        }
    }
}

impl Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

static CURRENT_METHOD: OnceLock<Method> = OnceLock::new();

pub struct MethodState;

impl MethodState {
    pub fn set(method: Method) -> Result<()> {
        let exist = |_| {
            const_concat! {
                CTX = "MethodState::set(): " + ONCE
            }
            Error::new(ErrorKind::AlreadyExists, CTX.as_str())
        };

        CURRENT_METHOD.set(method).map_err(exist)?;
        Ok(())
    }

    pub fn current() -> &'static Method {
        const_concat! {
            CTX = "MethodState::current(): " + INIT
        }
        CURRENT_METHOD.get().expect(&CTX)
    }
}
