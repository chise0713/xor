use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    sync::atomic::Ordering,
};

use log::error;
use tokio::sync::oneshot::Sender;

use crate::SHUTDOWN;

#[derive(Debug)]
pub struct AlignBox {
    ptr: NonNull<u8>,
    len: usize,
    layout: Layout,
}

unsafe impl Send for AlignBox {}
unsafe impl Sync for AlignBox {}

impl AlignBox {
    pub fn new(size: usize) -> Self {
        let layout = Layout::from_size_align(size, 64).unwrap();
        unsafe {
            let raw_ptr = alloc::alloc(layout);
            let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
            ptr::write_bytes(raw_ptr, 0, size);
            Self {
                ptr,
                len: size,
                layout,
            }
        }
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        unsafe {
            alloc::dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}

impl Deref for AlignBox {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for AlignBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

pub fn xor(tx: Sender<AlignBox>, mut buf: AlignBox, n: usize, token: u8) {
    #[inline(always)]
    fn token_to_u64(token: u8) -> u64 {
        let t = token as u64;
        t | (t << 8) | (t << 16) | (t << 24) | (t << 32) | (t << 40) | (t << 48) | (t << 56)
    }

    unsafe {
        if !(buf.as_ptr() as usize).is_multiple_of(64) {
            std::hint::unreachable_unchecked();
        }
    }

    let (prefix, middle, suffix) = unsafe { buf[..n].align_to_mut() };

    let token_u64 = token_to_u64(token);

    prefix.iter_mut().for_each(|b| *b ^= token);
    middle // longer align for simd
        .iter_mut()
        .for_each(|chunk: &mut u64| *chunk ^= token_u64);
    suffix.iter_mut().for_each(|b| *b ^= token);

    if tx.send(buf).is_err() {
        SHUTDOWN.store(true, Ordering::Relaxed);
        error!("receiver is dropped");
    };
}
