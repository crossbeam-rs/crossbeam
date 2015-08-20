use std::marker;
use std::cell::UnsafeCell;
use std::mem;
use std::ptr;
use std::ops::{Deref, DerefMut};

const CACHE_LINE: usize = 128;

// assume a cacheline size of CACHE_LINE bytes,
// and that T is smaller than a cacheline
pub struct CachePadded<T> {
    data: UnsafeCell<[u8; CACHE_LINE]>,
    _marker: marker::PhantomData<T>,
}

impl<T> CachePadded<T> {
    pub const fn zeroed() -> CachePadded<T> {
        CachePadded {
            data: UnsafeCell::new([0; CACHE_LINE]),
            _marker: marker::PhantomData,
        }
    }

    // safe to call only when sizeof(T) <= CACHE_LINE
    pub unsafe fn new(t: T) -> CachePadded<T> {
        let ret = CachePadded {
            //data: UnsafeCell::new(mem::uninitialized()),
            data: UnsafeCell::new([0; CACHE_LINE]),
            _marker: marker::PhantomData,
        };
        let p: *mut T = mem::transmute(&ret.data);
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cache_padded_store_u64() {
        let x: CachePadded<u64> = unsafe { CachePadded::new(17) };
        assert_eq!(*x, 17);
    }

    #[test]
    fn cache_padded_store_pair() {
        let x: CachePadded<(u64, u64)> = unsafe { CachePadded::new((17, 37)) };
        assert_eq!(x.0, 17);
        assert_eq!(x.1, 37);
    }
}
