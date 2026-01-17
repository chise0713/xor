use log::error;
use tokio::sync::oneshot::Sender;
use wide::u64x8;

use crate::{
    buf_pool::{AlignBox, CACHELINE_ALIGN},
    shutdown::Shutdown,
};

const EXBAND: usize = if cfg!(target_arch = "aarch64") { 8 } else { 4 };

#[inline(always)]
fn align_check(ptr: *const u8) {
    let ptr = ptr as usize;
    let aligned = ptr.is_multiple_of(CACHELINE_ALIGN);
    debug_assert!(aligned, "buf must be {}B aligned", CACHELINE_ALIGN);
    if !aligned {
        unsafe { std::hint::unreachable_unchecked() }
    }
}

#[inline(always)]
pub fn xor(tx: Sender<AlignBox>, mut buf: AlignBox, n: usize, token: u8) {
    align_check(buf.as_ptr());

    let (prefix, middle, suffix) = unsafe { buf[..n].align_to_mut() };
    let middle: &mut [u64x8] = middle;

    let simd = u64x8::splat(u64::from_ne_bytes([token; 8]));

    prefix.iter_mut().for_each(|b| *b ^= token);
    suffix.iter_mut().for_each(|b| *b ^= token);

    let mut chunks = middle.chunks_exact_mut(EXBAND);

    chunks
        .by_ref()
        .for_each(|chunk| (0..EXBAND).for_each(|i| chunk[i] ^= simd));
    chunks
        .into_remainder()
        .iter_mut()
        .for_each(|rem| *rem ^= simd);

    if tx.send(buf).is_err() && Shutdown::try_request() {
        error!("receiver is dropped");
    };
}
