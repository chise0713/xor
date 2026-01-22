use std::process::ExitCode;

use anyhow::Result;

#[derive(supershorty::Args, Debug)]
#[args(name = "xor")]
pub struct Args {
    #[arg(flag = 'b', help = "for total pre-allocated buffer")]
    pub buffer_limit_usize: Option<usize>,
    #[arg(flag = 'l', help = "listen address")]
    pub listen_address: Option<Box<str>>,
    #[arg(flag = 'm', help = "for link mtu")]
    pub mtu_usize: Option<usize>,
    #[arg(flag = 'r', help = "remote address")]
    pub remote_address: Option<Box<str>>,
    #[arg(flag = 'o', help = "client timeout in seconds")]
    pub timeout_f64_secs: Option<f64>,
    #[arg(flag = 't', help = "e.g. 0xFF")]
    pub token_hex_u8: Option<Box<str>>,
    #[arg(flag = 's', help = "one of: `xor`, `dns`. defaults to `xor`")]
    pub set_method: Option<Box<str>>,
}

const EXIT_INVALID_ARG: u8 = 2;

pub fn invalid_argument() -> Result<ExitCode> {
    Args::usage();
    Ok(ExitCode::from(EXIT_INVALID_ARG))
}
