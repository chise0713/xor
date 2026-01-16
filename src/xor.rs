use log::error;
use tokio::sync::oneshot::Sender;
use wide::u64x8;

use crate::{
    buf_pool::{AlignBox, CACHELINE_ALIGN},
    shutdown::Shutdown,
};

#[inline(always)]
pub fn xor(tx: Sender<AlignBox>, mut buf: AlignBox, n: usize, token: u8) {
    let ptr = buf.as_ptr() as usize;
    let aligned = ptr.is_multiple_of(CACHELINE_ALIGN);
    debug_assert!(aligned, "buf must be {}B aligned", CACHELINE_ALIGN);
    if !aligned {
        unsafe { std::hint::unreachable_unchecked() }
    }

    let (prefix, middle, suffix) = unsafe { buf[..n].align_to_mut() };
    let middle: &mut [u64x8] = middle;

    let simd = u64x8::splat(u64::from_ne_bytes([token; 8]));

    prefix.iter_mut().for_each(|b| *b ^= token);
    middle.iter_mut().for_each(|chunk| *chunk ^= simd);
    suffix.iter_mut().for_each(|b| *b ^= token);

    if tx.send(buf).is_err() {
        Shutdown::request();
        error!("receiver is dropped");
    };
}
