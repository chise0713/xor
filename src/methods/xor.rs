use std::{slice, sync::OnceLock};

use anyhow::Result;
use wide::u64x8;

use crate::{INIT, ONCE, buf_pool::SIMD_WIDTH, concat_let, methods::MethodImpl};

static TOKEN_U8: OnceLock<u8> = OnceLock::new();
static TOKEN_SIMD: OnceLock<u64x8> = OnceLock::new();

pub struct XorToken;

impl XorToken {
    pub fn set(val: u8) -> Result<()> {
        concat_let! {
            ctx = "XorToken::set()" + ONCE
        };
        TOKEN_U8.set(val).expect(&ctx);
        const BROADCAST_MUL: u64 = 0x0101010101010101;
        TOKEN_SIMD
            .set(u64x8::splat(BROADCAST_MUL.wrapping_mul(val as u64)))
            .expect(&ctx);
        Ok(())
    }

    #[must_use]
    #[inline(always)]
    fn get() -> (u8, u64x8) {
        concat_let! {
            ctx = "XorToken::get()" + INIT
        };
        (*TOKEN_U8.get().expect(&ctx), *TOKEN_SIMD.get().expect(&ctx))
    }
}

pub struct Xor;

impl MethodImpl for Xor {
    #[inline(always)]
    fn apply(ptr: *mut u8, n: &mut usize) {
        xor(ptr, n)
    }
}

#[inline(always)]
fn xor(ptr: *mut u8, n: &usize) {
    super::align_check(ptr.addr());
    let (token, simd) = XorToken::get();

    let n_simd = *n / SIMD_WIDTH;
    let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), n_simd) };

    data.iter_mut().for_each(|chunk| *chunk ^= simd);

    let rem = *n % SIMD_WIDTH;
    if rem == 0 {
        return;
    }
    let rem_ptr = unsafe { ptr.add(n_simd * SIMD_WIDTH) };

    let tail = unsafe { slice::from_raw_parts_mut(rem_ptr, rem) };
    tail.iter_mut().for_each(|byte| *byte ^= token);
}

#[cfg(all(test, feature = "bench"))]
mod bench {
    extern crate test;

    use test::Bencher;

    use super::XorToken;
    use crate::{K, buf_pool::AlignBox};

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
            super::xor(ptr, &n);
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
