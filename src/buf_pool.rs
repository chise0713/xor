// unsafe codes..
use std::{
    alloc::{self, Layout},
    cell::RefCell,
    io::{Error, ErrorKind},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::{OnceLock, atomic::Ordering},
};

use anyhow::Result;
use arrayvec::ArrayVec;
use wide::u64x8;

use self::sealed::BufPoolCell;
use crate::{INIT, ONCE, SLOW_DROP_COUNT, TASK_PER_THREAD, const_concat};

pub const SIMD_WIDTH: usize = size_of::<u64x8>();

pub struct AlignBox {
    ptr: NonNull<u8>,
    padded_len: usize,
}

impl AlignBox {
    #[inline(always)]
    const fn align() -> usize {
        SIMD_WIDTH
    }

    #[inline(always)]
    const fn padded_len(size: usize) -> usize {
        size.div_ceil(Self::align()) * Self::align()
    }

    fn layout(padded_len: usize) -> Layout {
        Layout::from_size_align(padded_len, Self::align()).unwrap()
    }

    pub fn new(size: usize) -> Self {
        let padded_len = Self::padded_len(size);
        let layout = Self::layout(padded_len);
        let raw_ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw_ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self { ptr, padded_len }
    }

    #[cfg(test)]
    pub(crate) const fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr.as_ptr()
    }
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.padded_len, Self::align()).unwrap();
        unsafe { alloc::dealloc(self.ptr.as_ptr(), layout) }
    }
}

thread_local! {
    /// small work-around of the sharing buffer pool across all threads
    /// still needs some refactor
    static META_IDX_POOL: RefCell<ArrayVec<usize, TASK_PER_THREAD>> = const { RefCell::new(ArrayVec::new_const()) };
}

pub struct BufPool;

impl BufPool {
    pub fn init(cap: usize, payload_max: usize) -> Result<()> {
        let stride = AlignBox::padded_len(payload_max);
        let slab = AlignBox::new(cap * stride);

        BufPoolCell::init(cap, slab, stride)?;
        Ok(())
    }

    pub fn acquire() -> LeasedBuf {
        if let Some(meta) = META_IDX_POOL.with_borrow_mut(ArrayVec::pop) {
            // # Safety
            // meta is from `META_IDX_POOL.pop()`
            return unsafe { BufPoolCell.meta_to_buf(meta) };
        };

        BufPoolCell.pop()
    }
}

pub struct LeasedBuf {
    ptr: NonNull<[u8]>,
    meta: usize,
}

impl LeasedBuf {
    #[cold]
    #[inline(never)]
    fn drop_slow(&self) {
        BufPoolCell.push(self.meta);
        SLOW_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

impl Deref for LeasedBuf {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl DerefMut for LeasedBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl Drop for LeasedBuf {
    fn drop(&mut self) {
        let return_to_global = META_IDX_POOL.with_borrow_mut(|pool| {
            if !pool.is_full() {
                pool.push(self.meta);
                false
            } else {
                true
            }
        });

        if return_to_global {
            self.drop_slow();
        }
    }
}

mod sealed {
    use std::ptr;

    use parking_lot::{Condvar, Mutex};

    use super::*;

    /// FIXME: currentlly built on top of sharing buffer
    /// across all threads, but it should be consider to
    /// make every thread has it's own buffer pool
    pub(super) struct BufSlabPool {
        available: Mutex<Vec<usize>>,
        cv: Condvar,
        slab: AlignBox,
        stride: usize,
    }

    impl BufSlabPool {
        fn new(cap: usize, slab: AlignBox, stride: usize) -> Self {
            Self {
                available: Mutex::new(Vec::from_iter(0..cap)),
                cv: Condvar::new(),
                slab,
                stride,
            }
        }

        pub(super) fn push(&self, meta: usize) {
            {
                let mut v = self.available.lock();

                debug_assert!(v.len() < v.capacity());

                v.push(meta);
            }
            self.cv.notify_one();
        }

        pub(super) fn pop(&self) -> LeasedBuf {
            let mut v = self.available.lock();
            while v.is_empty() {
                self.cv.wait(&mut v);
            }
            let meta = v.pop().unwrap();
            // # Safety
            // meta is from `available.pop()`
            unsafe { self.meta_to_buf(meta) }
        }

        // very unsafe
        pub(super) unsafe fn meta_to_buf(&self, meta: usize) -> LeasedBuf {
            let offset = meta * self.stride;

            debug_assert!(offset < self.slab.padded_len);

            let ptr = ptr::slice_from_raw_parts_mut(
                unsafe { self.slab.ptr.as_ptr().add(offset) },
                self.stride,
            );

            LeasedBuf {
                // # Safety
                // ptr comes from `self.slab.ptr`, which is a `NonNull<u8>`
                ptr: unsafe { NonNull::new_unchecked(ptr) },
                meta,
            }
        }
    }

    unsafe impl Send for BufSlabPool {}
    unsafe impl Sync for BufSlabPool {}

    static BUF_SLAB_POOL: OnceLock<BufSlabPool> = OnceLock::new();

    pub(super) struct BufPoolCell;

    impl Deref for BufPoolCell {
        type Target = BufSlabPool;

        fn deref(&self) -> &Self::Target {
            const_concat! {
                CTX = "BufPool: " + INIT
            };
            BUF_SLAB_POOL.get().expect(&CTX)
        }
    }

    impl BufPoolCell {
        pub(super) fn init(cap: usize, slab: AlignBox, stride: usize) -> Result<()> {
            BUF_SLAB_POOL
                .set(BufSlabPool::new(cap, slab, stride))
                .map_err(|_| {
                    const_concat! {
                        CTX = "BufPoolCell::init(): " + ONCE
                    };
                    Error::new(ErrorKind::AlreadyExists, CTX.as_str())
                })?;

            Ok(())
        }
    }
}
