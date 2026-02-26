// unsafe codes..
use std::{
    alloc::{self, Layout},
    cell::RefCell,
    io::{Error, ErrorKind},
    ops::{Deref, DerefMut},
    ptr::NonNull,
    slice,
    sync::OnceLock,
};

use anyhow::Result;
use arrayvec::ArrayVec;
use crossbeam_queue::ArrayQueue;
use crossbeam_utils::CachePadded;
use wide::u64x8;

use self::sealed::BufPoolCell;
use crate::{INIT, ONCE, TASK_PER_THREAD, const_concat};

pub const SIMD_WIDTH: usize = size_of::<u64x8>();

pub struct AlignBox {
    ptr: NonNull<u8>,
    padded_len: usize,
}

pub const CACHELINE_ALIGN: usize = size_of::<CachePadded<Box<()>>>();

impl AlignBox {
    #[inline(always)]
    fn align() -> usize {
        CACHELINE_ALIGN.max(SIMD_WIDTH)
    }

    #[inline(always)]
    fn padded_len(size: usize) -> usize {
        size.div_ceil(SIMD_WIDTH) * SIMD_WIDTH
    }

    #[inline(always)]
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
}

impl Drop for AlignBox {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.padded_len, Self::align()).unwrap();
        unsafe { alloc::dealloc(self.ptr.as_ptr(), layout) }
    }
}

impl Deref for AlignBox {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.padded_len) }
    }
}

impl DerefMut for AlignBox {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.padded_len) }
    }
}

const PUSH_FAILURE: &str = "failed to push BufPool";

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

    pub fn acquire() -> Option<LeasedBuf> {
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
        BufPoolCell.push(self.meta).expect(PUSH_FAILURE);
    }
}

unsafe impl Send for LeasedBuf {}

impl Deref for LeasedBuf {
    type Target = [u8];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl DerefMut for LeasedBuf {
    #[inline(always)]
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

    use super::*;

    /// FIXME: currentlly built on top of sharing buffer
    /// across all threads, but it should be consider to
    /// make every thread has it's own buffer pool
    pub(super) struct BufSlabPool {
        available: ArrayQueue<usize>,
        slab: AlignBox,
        stride: usize,
    }

    impl BufSlabPool {
        fn new(cap: usize, slab: AlignBox, stride: usize) -> Self {
            Self {
                available: ArrayQueue::new(cap),
                slab,
                stride,
            }
        }

        pub(super) fn push(&self, meta: usize) -> Result<(), usize> {
            self.available.push(meta)
        }

        pub(super) fn pop(&self) -> Option<LeasedBuf> {
            let meta = self.available.pop()?;
            // # Safety
            // meta is from `available.pop()`
            unsafe { self.meta_to_buf(meta) }
        }

        // very unsafe
        pub(super) unsafe fn meta_to_buf(&self, meta: usize) -> Option<LeasedBuf> {
            let offset = meta * self.stride;
            let ptr = ptr::slice_from_raw_parts_mut(
                unsafe { self.slab.ptr.as_ptr().add(offset) },
                self.stride,
            );
            NonNull::new(ptr).map(|ptr| LeasedBuf { ptr, meta })
        }
    }

    unsafe impl Send for BufSlabPool {}
    unsafe impl Sync for BufSlabPool {}

    static BUF_SLAB_POOL: OnceLock<BufSlabPool> = OnceLock::new();

    pub(super) struct BufPoolCell;

    impl Deref for BufPoolCell {
        type Target = BufSlabPool;

        #[inline(always)]
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

            (0..cap).for_each(|i| {
                BufPoolCell.push(i).expect(PUSH_FAILURE);
            });

            Ok(())
        }
    }
}
