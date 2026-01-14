use log::error;
use tokio::sync::oneshot::Sender;

use crate::{buf_pool::AlignBox, shutdown};

pub fn xor(tx: Sender<AlignBox>, mut buf: AlignBox, n: usize, token: u8) {
    #[inline(always)]
    fn token_to_u64(token: u8) -> u64 {
        let t = token as u64;
        t | (t << 8) | (t << 16) | (t << 24) | (t << 32) | (t << 40) | (t << 48) | (t << 56)
    }

    unsafe {
        if !(buf.as_ptr() as usize).is_multiple_of(64) {
            std::hint::unreachable_unchecked();
        }
    }

    let (prefix, middle, suffix) = unsafe { buf[..n].align_to_mut() };

    let token_u64 = token_to_u64(token);

    prefix.iter_mut().for_each(|b| *b ^= token);
    middle // longer align for simd
        .iter_mut()
        .for_each(|chunk: &mut u64| *chunk ^= token_u64);
    suffix.iter_mut().for_each(|b| *b ^= token);

    if tx.send(buf).is_err() {
        shutdown::set(true);
        error!("receiver is dropped");
    };
}
