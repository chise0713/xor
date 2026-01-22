use std::{str::FromStr, sync::OnceLock};

use anyhow::bail;

use crate::{INIT, ONCE, concat_let};

macro_rules! import_modules {
    ($($mod_name:ident),*) => {
        $(
            mod $mod_name;
            pub use $mod_name::*;
        )*
    };
}

#[inline(always)]
fn align_check(ptr: usize) {
    use crate::buf_pool::CACHELINE_ALIGN;

    if !ptr.is_multiple_of(CACHELINE_ALIGN) {
        unreachable!("buf must be {}B aligned", CACHELINE_ALIGN)
    }
}

#[repr(u8)]
#[derive(Default)]
pub enum Method {
    #[default]
    Xor,
    DnsPad,
    DnsUnPad,
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

static CURRENT_METHOD: OnceLock<Method> = OnceLock::new();

pub struct MethodState;

impl MethodState {
    pub fn set(method: Method) {
        concat_let! {
            ctx = "MethodState: " + ONCE
        }
        CURRENT_METHOD.set(method).ok().expect(&ctx)
    }

    pub fn current() -> &'static Method {
        concat_let! {
            ctx = "MethodState: " + INIT
        }
        CURRENT_METHOD.get().expect(&ctx)
    }
}

import_modules!(xor, dns);
