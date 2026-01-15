use std::io::Write as _;

use env_logger::{Env, Target};
use log::Level;

pub struct Logger;

impl Logger {
    pub fn init() {
        env_logger::Builder::from_env(Env::default().default_filter_or("info"))
            .target(Target::Stdout)
            .format(move |buf, record| {
                let level_str = match record.level() {
                    Level::Trace => "\x1B[1;35mTRACE\x1B[0m",
                    Level::Debug => "\x1B[1;30mDEBUG\x1B[0m",
                    Level::Info => "\x1B[1;36mINFO\x1B[0m",
                    Level::Warn => "\x1B[1;93mWARN\x1B[0m",
                    Level::Error => "\x1B[1;31mERROR\x1B[0m",
                };
                writeln!(
                    buf,
                    "[{} {}:{} {}]: {}",
                    buf.timestamp_millis(),
                    record.file().unwrap(),
                    record.line().unwrap(),
                    level_str,
                    record.args()
                )
            })
            .init()
    }
}
