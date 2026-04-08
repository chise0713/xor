use core::slice;
use std::{
    io::{Error, ErrorKind},
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use wide::{u64x2, u64x4, u64x8};

use super::MethodApply;
use crate::{INIT, ONCE, buf_pool::SIMD_WIDTH, const_concat};

static TOKEN: AtomicUsize = AtomicUsize::new(XorToken::SENTINEL);

pub struct XorToken;

impl XorToken {
    const SENTINEL: usize = 1 << 8;

    pub fn init(val: u8) -> Result<()> {
        if TOKEN
            .compare_exchange(
                Self::SENTINEL,
                val as usize,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_err()
        {
            const_concat! {
                CTX = "XorToken::set()" + ONCE
            };
            return Err(Error::new(ErrorKind::AlreadyExists, CTX.as_str()))?;
        };
        Ok(())
    }

    #[must_use]
    #[inline]
    fn get() -> u8 {
        const_concat! {
            CTX = "XorToken::get()" + INIT
        };

        let val = TOKEN.load(Ordering::Acquire);

        if val == Self::SENTINEL {
            panic!("{}", CTX);
        }

        val as u8
    }
}

pub struct Xor;

impl MethodApply for Xor {
    #[inline(always)]
    fn apply(buf: &mut [u8], n: &mut usize) -> Result<()> {
        let token = XorToken::get();
        unsafe { xor(buf.as_mut_ptr(), *n, token) }
        Ok(())
    }
}

unsafe fn xor(ptr: *mut u8, n: usize, token: u8) {
    let aligned = ptr.addr().is_multiple_of(SIMD_WIDTH);

    if !aligned {
        unreachable!("buf must be {}B aligned", SIMD_WIDTH)
    }

    if token == 0 || n == 0 {
        return;
    }

    let token64 = u64::from_ne_bytes([token; _]);

    let token_simd_512 = u64x8::splat(token64);

    const CHUNK_SIZE: usize = size_of::<u64x8>();

    let chunks = n / CHUNK_SIZE;

    if chunks != 0 {
        // # Safety
        // `ptr` is checked by `super::align_check()`
        let data: &mut [u64x8] = unsafe { slice::from_raw_parts_mut(ptr.cast(), chunks) };

        data.iter_mut().for_each(|chunk| *chunk ^= token_simd_512);
    }

    let count = chunks * CHUNK_SIZE;
    let base = unsafe { ptr.add(count) };
    let tail = n - count;
    if tail == 0 {
        return;
    }

    // cascading branches, tail decomposition
    macro_rules! xor_tail {
        // core
        (@core $off:ident, $t:ty, $token:expr, $advance:expr) => {
            if tail & ::core::mem::size_of::<$t>() != 0 {
                let v: *mut $t = unsafe { base.add($off) }.cast();
                unsafe { *v ^= $token };
                #[cfg($advance)]
                {
                    $off += ::core::mem::size_of::<$t>();
                }
            }
        };

        // last
        (@step $off:ident, $t:ty => $token:expr;) => {
            xor_tail!(@core $off, $t, $token, false);
        };

        // step
        (@step $off:ident, $t:ty => $token:expr; $($rest:tt)+) => {
            xor_tail!(@core $off, $t, $token, true);
            xor_tail!(@step $off, $($rest)+);
        };

        // enter
        ($($tt:tt)+) => {
            let mut off = 0;
            xor_tail!(@step off, $($tt)+);
        };
    }

    xor_tail! {
        u64x4 => u64x4::splat(token64);
        u64x2 => u64x2::splat(token64);
        u64   => token64;
        u32   => token64 as u32;
        u16   => token64 as u16;
        u8    => token;
    }
}

#[test]
fn test_xor_roundtrip() {
    use crate::buf_pool::AlignBox;

    const SIZE: usize = 256;

    let mut buf = AlignBox::new(SIZE);
    let buf = unsafe { slice::from_raw_parts_mut(buf.as_mut_ptr(), SIZE) };

    const TOKEN: u8 = 0xAA;

    XorToken::init(TOKEN).unwrap();

    let payload: Box<[u8]> = (0..SIZE).map(|x| x as u8).collect();
    let mut n = payload.len();

    buf[..n].copy_from_slice(&payload);
    let original = payload.clone();
    let normal_xor = {
        let mut payload = payload.clone();
        payload.iter_mut().for_each(|b| *b ^= TOKEN);
        payload
    };

    Xor::apply(buf, &mut n).unwrap();

    assert_ne!(&buf[..n], &*original);
    assert_eq!(&buf[..n], &*normal_xor);

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
