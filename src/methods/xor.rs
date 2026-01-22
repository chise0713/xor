use std::{slice, sync::OnceLock};

use anyhow::Result;
use wide::u64x8;

use crate::{INIT, ONCE, buf_pool::SIMD_WIDTH, concat_let};

static TOKEN: OnceLock<u8> = OnceLock::new();

pub struct XorToken;

impl XorToken {
    pub fn set(val: u8) -> Result<()> {
        concat_let! {
            ctx = "XorToken::set()" + ONCE
        };
        TOKEN.set(val).expect(&ctx);
        Ok(())
    }

    #[must_use]
    #[inline(always)]
    pub(super) fn get() -> u8 {
        concat_let! {
            ctx = "XorToken::get()" + INIT
        };
        *TOKEN.get().expect(&ctx)
    }
}

#[inline(always)]
#[must_use]
pub fn xor(ptr: *mut u8, n: usize) -> usize {
    super::align_check(ptr.addr());
    let token = XorToken::get();

    let n_simd = n / SIMD_WIDTH;
    let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), n_simd) };

    const BROADCAST_MUL: u64 = 0x0101010101010101;
    let simd = u64x8::splat(BROADCAST_MUL.wrapping_mul(token as u64));

    data.iter_mut().for_each(|chunk| *chunk ^= simd);

    let rem = n % SIMD_WIDTH;
    if rem == 0 {
        return n;
    }
    let rem_ptr = unsafe { ptr.add(n_simd * SIMD_WIDTH) };

    let tail = unsafe { slice::from_raw_parts_mut(rem_ptr, rem) };
    tail.iter_mut().for_each(|byte| *byte ^= token);

    n
}

#[cfg(all(test, feature = "bench"))]
mod bench {
    extern crate test;

    use test::Bencher;

    use crate::{K, buf_pool::AlignBox, methods::XorToken};

    const TEST_ITER: usize = K;

    const N: usize = 16 * K;
    const TOKEN: u8 = 0xFF;

    #[inline(always)]
    fn bench(ptr: *mut u8, xor: fn(*mut u8, usize, u8)) {
        use std::{
            hint,
            sync::{atomic, atomic::Ordering},
        };
        // pin LLVM optimization path to prevent cross-call folding
        let pin = || {
            hint::black_box(ptr);
            atomic::compiler_fence(Ordering::SeqCst);
        };
        (0..TEST_ITER).for_each(|_| {
            xor(ptr, N, TOKEN);
            pin();
            xor(ptr, N, TOKEN);
            pin();
        })
    }

    #[bench]
    fn simd(b: &mut Bencher) {
        #[inline(always)]
        fn xor(ptr: *mut u8, n: usize, _: u8) {
            _ = super::xor(ptr, n);
        }
        let mut data = AlignBox::new(N);
        let ptr = data.as_mut_ptr();
        XorToken::set(TOKEN).unwrap();
        b.iter(|| bench(ptr, xor));
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
        b.iter(|| bench(ptr, xor));
    }
}
