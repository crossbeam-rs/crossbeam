use std::marker;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem;
use std::ptr;
use std::ops::{Deref, DerefMut};

// x86_64 has a 64byte cache alignment, so set this to 8 * sizeof(usize)
// which happens to be 8, ie 64 bytes. However, the tests fail, so we set
// this to 16 for 128 bytes to allow dwcas (which is likely the issue)
#[cfg(target_arch = "x86_64")]
const CACHE_LINE: usize = 16;
// Other arches may not - for example, x86 may vary, ppc64 is 128, ppc32 is 32.
// To be safe, we set a minimum of 32 * sizeof(usize) here, but specific
// processor versions could be added later.
#[cfg(not(target_arch = "x86_64"))]
const CACHE_LINE: usize = 32;

#[cfg_attr(feature = "nightly",
           repr(simd))]
#[derive(Debug)]
struct Padding(u64, u64, u64, u64);

/// Pad `T` to the length of a cacheline.
///
/// Sometimes concurrent programming requires a piece of data to be padded out
/// to the size of a cacheline to avoid "false sharing": cachelines being
/// invalidated due to unrelated concurrent activity. Use the `CachePadded` type
/// when you want to *avoid* cache locality.
///
/// At the moment, cache lines are assumed to be 32 * sizeof(usize) on all
/// architectures.
///
/// **Warning**: the wrapped data is never dropped; move out using `ptr::read`
/// if you need to run dtors.
pub struct CachePadded<T> {
    data: UnsafeCell<[usize; CACHE_LINE]>,
    _marker: ([Padding; 0], marker::PhantomData<T>),
}

impl<T> fmt::Debug for CachePadded<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CachePadded {{ ... }}")
    }
}

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

#[cfg(not(feature = "nightly"))]
macro_rules! declare_zeros_valid {
    () => {
        /// Types for which mem::zeroed() is safe.
        ///
        /// If a type `T: ZerosValid`, then a sequence of zeros the size of `T` must be
        /// a valid member of the type `T`.
        pub unsafe trait ZerosValid {}
    }
}

#[cfg(feature = "nightly")]
macro_rules! declare_zeros_valid {
    () => {
        /// Types for which mem::zeroed() is safe.
        ///
        /// If a type `T: ZerosValid`, then a sequence of zeros the size of `T` must be
        /// a valid member of the type `T`.
        pub unsafe auto trait ZerosValid {}
    }
}

declare_zeros_valid!();

macro_rules! zeros_valid { ($( $T:ty )*) => ($(
    unsafe impl ZerosValid for $T {}
)*)}

zeros_valid!(u8 u16 u32 u64 usize);
zeros_valid!(i8 i16 i32 i64 isize);

unsafe impl ZerosValid for ::std::sync::atomic::AtomicUsize {}
unsafe impl<T> ZerosValid for ::std::sync::atomic::AtomicPtr<T> {}

impl<T: ZerosValid> CachePadded<T> {
    /// A const fn equivalent to mem::zeroed().
    #[cfg(not(feature = "nightly"))]
    pub fn zeroed() -> CachePadded<T> {
        CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _marker: ([], marker::PhantomData),
        }
    }

    /// A const fn equivalent to mem::zeroed().
    #[cfg(feature = "nightly")]
    pub const fn zeroed() -> CachePadded<T> {
        CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _marker: ([], marker::PhantomData),
        }
    }
}

#[inline]
/// Assert that the size and alignment of `T` are consistent with `CachePadded<T>`.
fn assert_valid<T>() {
    assert!(mem::size_of::<T>() <= mem::size_of::<CachePadded<T>>());
    assert!(mem::align_of::<T>() <= mem::align_of::<CachePadded<T>>());
}

impl<T> CachePadded<T> {
    /// Wrap `t` with cacheline padding.
    ///
    /// **Warning**: the wrapped data is never dropped; move out using
    /// `ptr:read` if you need to run dtors.
    pub fn new(t: T) -> CachePadded<T> {
        assert_valid::<T>();
        let ret = CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _marker: ([], marker::PhantomData),
        };
        unsafe {
            let p: *mut T = mem::transmute(&ret.data);
            ptr::write(p, t);
        }
        ret
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        assert_valid::<T>();
        unsafe { mem::transmute(&self.data) }
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        assert_valid::<T>();
        unsafe { mem::transmute(&mut self.data) }
    }
}

// FIXME: support Drop by pulling out a version usable for statics
/*
impl<T> Drop for CachePadded<T> {
    fn drop(&mut self) {
        assert_valid::<T>();
        let p: *mut T = mem::transmute(&self.data);
        mem::drop(ptr::read(p));
    }
}
*/

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cache_padded_store_u64() {
        let x: CachePadded<u64> = CachePadded::new(17);
        assert_eq!(*x, 17);
    }

    #[test]
    fn cache_padded_store_pair() {
        let x: CachePadded<(u64, u64)> = CachePadded::new((17, 37));
        assert_eq!(x.0, 17);
        assert_eq!(x.1, 37);
    }
}
