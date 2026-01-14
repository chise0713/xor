use std::{
    alloc::{self, Layout},
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
};

use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use tokio::sync::OnceCell;

use crate::POOL_SEM;

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

static BUF_POOL: OnceCell<ArrayQueue<AlignBox>> = OnceCell::const_new();

pub fn init(limit: usize, payload_max: usize) -> Result<()> {
    BUF_POOL.set(ArrayQueue::new(limit))?;
    (0..limit).for_each({
        let pool = BUF_POOL.get().unwrap();
        |_| pool.push(AlignBox::new(payload_max)).unwrap()
    });
    POOL_SEM.add_permits(limit);
    Ok(())
}

pub fn get() -> &'static ArrayQueue<AlignBox> {
    BUF_POOL.get().unwrap()
}
