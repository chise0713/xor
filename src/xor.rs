use std::slice;

use wide::u64x8;

use crate::buf_pool::{CACHELINE_ALIGN, SIMD_WIDTH};

#[inline(always)]
fn align_check(ptr: usize) {
    if !ptr.is_multiple_of(CACHELINE_ALIGN) {
        unreachable!("buf must be {}B aligned", CACHELINE_ALIGN)
    }
}

#[inline(always)]
pub fn xor(ptr: *mut u8, n: usize, token: u8) {
    align_check(ptr as usize);

    let n_simd = n / SIMD_WIDTH;
    let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), n_simd) };

    const BROADCAST_MUL: u64 = 0x0101010101010101;
    let simd = u64x8::splat(BROADCAST_MUL.wrapping_mul(token as u64));

    data.iter_mut().for_each(|chunk| *chunk ^= simd);

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

#[cfg(all(test, feature = "bench"))]
mod bench {
    extern crate test;

    use test::Bencher;

    use super::xor;
    use crate::{K, buf_pool::AlignBox};

    const TEST_ITER: usize = K;

    const N: usize = 16 * K;
    const TOKEN: u8 = 0xFF;

    #[bench]
    fn simd(b: &mut Bencher) {
        let mut data = AlignBox::new(N);
        let ptr = data.as_mut_ptr();
        b.iter(|| {
            (0..TEST_ITER).for_each(|_| {
                xor(ptr, N, TOKEN);
                xor(ptr, N, TOKEN);
            });
        });
    }

    #[bench]
    fn normal(b: &mut Bencher) {
        #[inline(always)]
        fn xor(ptr: *mut u8, n: usize, token: u8) {
            (0..n).for_each(|i| unsafe {
                *ptr.add(i) ^= token;
            });
        }
        let mut data = unsafe { Box::new_zeroed_slice(N).assume_init() };
        let ptr: *mut u8 = data.as_mut_ptr();
        b.iter(|| {
            (0..TEST_ITER).for_each(|_| {
                xor(ptr, N, TOKEN);
                xor(ptr, N, TOKEN);
            });
        });
    }
}
