use std::marker;
use std::cell::UnsafeCell;
use std::mem;
use std::ptr;
use std::ops::{Deref, DerefMut};

// assume a cacheline size of 128 bytes, and that T is smaller than a cacheline
pub struct CachePadded<T> {
    data: UnsafeCell<[u8; 128]>,
    _marker: marker::PhantomData<T>,
}

impl<T> CachePadded<T> {
    pub const fn zeroed() -> CachePadded<T> {
        CachePadded {
            data: UnsafeCell::new([0; 128]),
            _marker: marker::PhantomData,
        }
    }

    // safe to call only when sizeof(T) <= 64
    pub unsafe fn new(t: T) -> CachePadded<T> {
        let ret = CachePadded {
            data: UnsafeCell::new(mem::uninitialized()),
            _marker: marker::PhantomData,
        };
        let p: *mut T = mem::transmute(&ret);
        ptr::write(p, t);
        ret
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { mem::transmute(&self.data) }
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { mem::transmute(&mut self.data) }
    }
}
