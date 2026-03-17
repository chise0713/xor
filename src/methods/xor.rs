use core::slice;
use std::sync::OnceLock;

use anyhow::Result;
use wide::{u64x4, u64x8};

use super::{ApplyProof, MethodApply};
use crate::{INIT, ONCE, const_concat};

static TOKEN: OnceLock<u8> = OnceLock::new();

pub struct XorToken;

impl XorToken {
    pub fn init(val: u8) -> Result<()> {
        const_concat! {
            CTX = "XorToken::set()" + ONCE
        };
        TOKEN.set(val).expect(&CTX);
        Ok(())
    }

    #[must_use]
    #[inline(always)]
    fn get() -> u8 {
        const_concat! {
            CTX = "XorToken::get()" + INIT
        };
        *TOKEN.get().expect(&CTX)
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
    #[inline(always)]
    unsafe fn apply_unsafe<P>(_proof: P, ptr: *mut u8, n: &mut usize)
    where
        P: super::ApplyProof<Method = Self>,
    {
        let token = XorToken::get();
        unsafe { xor(ptr, *n, token) }
    }

    #[inline(always)]
    fn apply(buf: &mut [u8], n: &mut usize) -> Result<()> {
        let proof = Self::check_apply().unwrap();
        unsafe { Self::apply_unsafe(proof, buf.as_mut_ptr(), n) };
        Ok(())
    }
}

#[inline(always)]
unsafe fn xor(ptr: *mut u8, n: usize, token: u8) {
    super::align_check(ptr.addr());

    if token == 0 {
        return;
    }

    let t128 = u128::from_ne_bytes([token; _]);
    let t64 = u64::from_ne_bytes([token; _]);
    let t32 = u32::from_ne_bytes([token; _]);
    let t16 = u16::from_ne_bytes([token; _]);

    let simd8 = u64x8::splat(t64);

    const CHUNK_SIZE: usize = size_of::<u64x8>();

    let chunks = n / CHUNK_SIZE;

    if chunks != 0 {
        let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), chunks) };

        data.iter_mut().for_each(|chunk| *chunk ^= simd8);
    }

    let base = unsafe { ptr.add(chunks * CHUNK_SIZE) };
    let mut off = 0;
    let tail = n - chunks * CHUNK_SIZE;

    // branchless tail
    macro_rules! xor_tail {
        ($size:expr, $t:ty, $token:expr) => {
            if tail & $size != 0 {
                let v: *mut $t = unsafe { base.add(off) }.cast();
                unsafe { *v ^= $token };
                off += $size;
            }
        };
    }

    xor_tail!(size_of::<u64x4>(), u64x4, u64x4::splat(t64));
    xor_tail!(size_of::<u128>(), u128, t128);
    xor_tail!(size_of::<u64>(), u64, t64);
    xor_tail!(size_of::<u32>(), u32, t32);
    xor_tail!(size_of::<u16>(), u16, t16);
    xor_tail!(size_of::<u8>(), u8, token);

    _ = off;
}

#[test]
fn test_xor_roundtrip() {
    use crate::buf_pool::AlignBox;

    let size = 256;

    let mut buf = AlignBox::new(size);
    let buf = unsafe { slice::from_raw_parts_mut(buf.as_mut_ptr(), size) };

    XorToken::init(0xAA).unwrap();

    let payload: Box<[u8]> = (0..size).map(|x| x as u8).collect();
    let mut n = payload.len();

    buf[..n].copy_from_slice(&payload);
    let original = payload.clone();

    Xor::apply(buf, &mut n).unwrap();

    assert_ne!(&buf[..n], &*original);

    Xor::apply(buf, &mut n).unwrap();

    assert_eq!(n, original.len());

    assert_eq!(&buf[..n], &*original);
}

#[cfg(all(test, feature = "bench"))]
mod bench {
    // NOTE: SIMD path uses AlignBox intentionally to reflect real-world
    // aligned allocation requirements. This benchmark measures end-to-end
    // cost/benefit rather than pure instruction throughput.
    extern crate test;

    use test::Bencher;

    use crate::{K, buf_pool::AlignBox};

    const N: usize = 16 * K;
    const TOKEN: u8 = 0xFF;

    #[bench]
    fn simd(b: &mut Bencher) {
        let mut data = AlignBox::new(N);
        let ptr = data.as_mut_ptr();
        b.iter(|| unsafe { super::xor(ptr, N, TOKEN) });
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
