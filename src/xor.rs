use std::slice;

use wide::u64x8;

use crate::buf_pool::{CACHELINE_ALIGN, SIMD_WIDTH};

#[inline(always)]
fn align_check(ptr: *const u8) {
    let ptr = ptr as usize;
    let aligned = ptr.is_multiple_of(CACHELINE_ALIGN);
    debug_assert!(aligned, "buf must be {}B aligned", CACHELINE_ALIGN);
    if !aligned {
        unreachable!()
    }
}

pub fn xor(ptr: *mut u8, n: usize, token: u8) {
    align_check(ptr);

    let n_simd = n / SIMD_WIDTH;
    let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), n_simd) };

    let simd = u64x8::splat(u64::from_ne_bytes([token; 8]));

    data.iter_mut().for_each(|chunk| {
        *chunk ^= simd;
    });

    let rem = n % SIMD_WIDTH;
    if rem == 0 {
        return;
    }
    let rem_ptr = unsafe { ptr.add(n_simd * SIMD_WIDTH) };

    let tail = unsafe { slice::from_raw_parts_mut(rem_ptr, rem) };
    tail.iter_mut().for_each(|byte| {
        *byte ^= token;
    })
}
