use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::{ptr, mem, fmt, marker};

/// An over-approximation of ((cache-line-size) / sizeof(usize))
// FIXME: arch-dependent value for CACHE_LINE
const CACHE_LINE: usize = 32;

// FIXME: this is a spooky hack that aligns `CachePadded` to the cache line.
// It would be better to have a language feature on the alignment, e.g.
// `#[align(64)]`.  See https://github.com/rust-lang/rfcs/issues/325 for more
// details.
#[cfg_attr(feature = "nightly", repr(simd))]
#[derive(Debug)]
struct Padding([usize; CACHE_LINE]);

/// Pad `T` to the length of a cacheline.
///
/// Sometimes concurrent programming requires a piece of data to be padded out
/// to the size of a cacheline to avoid "false sharing": cachelines being
/// invalidated due to unrelated concurrent activity. Use the `CachePadded` type
/// when you want to *avoid* cache locality.
///
/// # Warning
///
/// - The wrapped data is never dropped; move out using `ptr::read` if you need
///   to run dtors.
/// - Do not rely on this actually being padded to the cache line for the
///   correctness of your program.  Certain compiler options or hardware might
///   actually cause it not to be aligned.
// FIXME: currently we require sizeof(T) <= cache line size.
pub struct CachePadded<T> {
    data: UnsafeCell<[usize; CACHE_LINE]>,
    _pad: [Padding; 0],
    _marker: marker::PhantomData<T>,
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

macro_rules! init_zero {
    () => ({
        assert_valid::<T>();
        CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _pad: [],
            _marker: marker::PhantomData,
        }}
    )
}

impl<T: ZerosValid> CachePadded<T> {
    /// A const fn equivalent to mem::zeroed().
    #[cfg(not(feature = "nightly"))]
    pub fn zeroed() -> CachePadded<T> {
        init_zero!()
    }

    /// A const fn equivalent to mem::zeroed().
    #[cfg(feature = "nightly")]
    pub const fn zeroed() -> CachePadded<T> {
        init_zero!()
    }
}

#[inline]
/// Assert that the size and alignment of `T` are consistent with `CachePadded<T>`.
fn assert_valid<T>() {
    assert!(mem::size_of::<T>() <= mem::size_of::<CachePadded<T>>());
    assert!(mem::align_of::<T>() <= mem::align_of::<CachePadded<T>>());
    assert_eq!(mem::size_of::<CachePadded<T>>(), CACHE_LINE * mem::size_of::<usize>());

    // FIXME: we should ensure that the alignment of `CachePadded<T>`
    // is a multiple of the cache line size, but
    // `mem::align_of::<CachePadded<T>>()` gives us a very small
    // number...
}

impl<T> CachePadded<T> {
    /// Wrap `t` with cacheline padding.
    ///
    /// # Warning
    ///
    /// The wrapped data is never dropped; move out using `ptr:read` if you need to run dtors.
    ///
    /// # Panic
    ///
    /// If `T` is bigger than a cache line, this will hit an assertion.
    pub fn new(t: T) -> CachePadded<T> {
        let ret = init_zero!();

        // Copy the data into the untyped buffer.
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
        unsafe { mem::transmute(&self.data) }
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { mem::transmute(&mut self.data) }
    }
}

// FIXME: support Drop by pulling out a version usable for statics
/*
impl<T> Drop for CachePadded<T> {
    fn drop(&mut self) {
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
