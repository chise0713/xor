use std::slice;

use log::error;
use tokio::sync::oneshot::Sender;
use wide::u64x8;

use crate::{
    buf_pool::{AlignBox, CACHELINE_ALIGN, SIMD_WIDTH},
    shutdown::Shutdown,
};

#[inline(always)]
fn align_check(ptr: *const u8) {
    let ptr = ptr as usize;
    let aligned = ptr.is_multiple_of(CACHELINE_ALIGN);
    debug_assert!(aligned, "buf must be {}B aligned", CACHELINE_ALIGN);
    if !aligned {
        unreachable!()
    }
}

#[inline(always)]
pub fn xor(tx: Sender<AlignBox>, mut buf: AlignBox, n: usize, token: u8) {
    let ptr = buf.as_mut_ptr();
    align_check(ptr);

    let n_aligned = n.div_ceil(64);
    let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), n_aligned) };

    let simd = u64x8::splat(u64::from_ne_bytes([token; 8]));

    let stride = (CACHELINE_ALIGN.saturating_div(SIMD_WIDTH)).max(1);

    let mut chunks = data.chunks_exact_mut(stride);
    chunks.by_ref().for_each(|chunk| {
        (0..stride).for_each(|i| chunk[i] ^= simd);
    });
    chunks
        .into_remainder()
        .iter_mut()
        .for_each(|single| *single ^= simd);

    if tx.send(buf).is_err() && Shutdown::try_request() {
        error!("receiver is dropped");
    };
}
