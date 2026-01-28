use std::{slice, sync::OnceLock};

use anyhow::Result;
use wide::u64x8;

use super::{ApplyProof, MethodApply};
use crate::{INIT, ONCE, buf_pool::SIMD_WIDTH, const_concat};

static TOKEN_U8: OnceLock<u8> = OnceLock::new();
static TOKEN_SIMD: OnceLock<u64x8> = OnceLock::new();

pub struct XorToken;

impl XorToken {
    pub fn init(val: u8) -> Result<()> {
        const_concat! {
            CTX = "XorToken::set()" + ONCE
        };
        TOKEN_U8.set(val).expect(&CTX);
        const BROADCAST_MUL: u64 = 0x0101010101010101;
        TOKEN_SIMD
            .set(u64x8::splat(BROADCAST_MUL.wrapping_mul(val as u64)))
            .expect(&CTX);
        Ok(())
    }

    #[must_use]
    #[inline(always)]
    fn get() -> (u8, u64x8) {
        const_concat! {
            CTX = "XorToken::get()" + INIT
        };
        (*TOKEN_U8.get().expect(&CTX), *TOKEN_SIMD.get().expect(&CTX))
    }
}

// ZST proof token with private field,
// can only be constructed by the module
mod proof {
    use super::*;

    pub struct XorApplyProof {
        _token: (),
    }

    impl ApplyProof for XorApplyProof {
        type Method = Xor;
    }

    impl Xor {
        pub fn check_apply() -> Option<XorApplyProof> {
            Some(XorApplyProof { _token: () })
        }
    }
}

pub struct Xor;

impl MethodApply for Xor {
    unsafe fn apply<P>(_proof: P, ptr: *mut u8, n: &mut usize)
    where
        P: super::ApplyProof<Method = Self>,
    {
        unsafe { xor(ptr, *n) }
    }
}

#[inline(always)]
unsafe fn xor(ptr: *mut u8, n: usize) {
    super::align_check(ptr.addr());
    let (token, simd) = XorToken::get();
    if token == 0 {
        return;
    }

    let n_simd = n / SIMD_WIDTH;
    let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), n_simd) };

    data.iter_mut().for_each(|chunk| *chunk ^= simd);

    let rem = n % SIMD_WIDTH;
    if rem == 0 {
        return;
    }
    let rem_ptr = unsafe { ptr.add(n_simd * SIMD_WIDTH) };

    let tail = unsafe { slice::from_raw_parts_mut(rem_ptr, rem) };
    tail.iter_mut().for_each(|byte| *byte ^= token);
}

#[cfg(all(test, feature = "bench"))]
mod bench {
    // NOTE: SIMD path uses AlignBox intentionally to reflect real-world
    // aligned allocation requirements. This benchmark measures end-to-end
    // cost/benefit rather than pure instruction throughput.
    extern crate test;

    use test::Bencher;

    use super::XorToken;
    use crate::{K, buf_pool::AlignBox};

    const N: usize = 16 * K;
    const TOKEN: u8 = 0xFF;

    #[bench]
    fn simd(b: &mut Bencher) {
        let mut data = AlignBox::new(N);
        let ptr = data.as_mut_ptr();
        XorToken::init(TOKEN).unwrap();
        b.iter(|| unsafe { super::xor(ptr, N) });
    }

    #[bench]
    fn normal(b: &mut Bencher) {
        let mut data = unsafe { Box::new_zeroed_slice(N).assume_init() };
        let ptr: *mut u8 = data.as_mut_ptr();
        b.iter(|| {
            (0..N).for_each(|i| unsafe { *ptr.add(i) ^= TOKEN });
        });
    }
}
