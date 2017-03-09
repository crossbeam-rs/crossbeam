//! Cache-padded values.

use std::cell::UnsafeCell;
use std::ops::{Deref, DerefMut};
use std::{ptr, mem, fmt, marker};

/// The size of a cache line.
///
/// Do not let your code's safety or correctness depend on this being the real size. The actual
/// value may not be equal to this.
// TODO: For now, treat this as an arch-independent constant.
const CACHE_LINE: usize = 64;

/// A type with memory align of cache line size.
#[cfg_attr(feature = "nightly", repr(simd))]
#[derive(Debug)]
struct Padding(u64, u64, u64, u64, u64, u64, u64, u64);

/// A type which is always aligned to cache line.
// TODO: Spooky hack.
type Pad = [Padding; 0];

/// Types for which mem::zeroed() is safe.
///
/// If a type `T: ZerosValid`, then a sequence of zeros the size of `T` must be a valid member of
/// the type `T`.
///
/// It also asserts that the type is compatible with `CachePadded` (i.e. its size and align are not
/// too big).
pub unsafe trait ZerosValid {}

#[cfg(feature = "nightly")]
unsafe impl ZerosValid for .. {}

macro_rules! zeros_valid { ($( $T:ty )*) => ($(
    unsafe impl ZerosValid for $T {}
)*)}

zeros_valid!(u8 u16 u32 u64 usize);
zeros_valid!(i8 i16 i32 i64 isize);

unsafe impl ZerosValid for ::std::sync::atomic::AtomicUsize {}
unsafe impl<T> ZerosValid for ::std::sync::atomic::AtomicPtr<T> {}

/// Assert that the size and alignment of `T` are consistent with `CachePadded<T>`.
fn assert_valid<T>() {
    assert!(mem::size_of::<T>() <= mem::size_of::<CachePadded<T>>());
    assert!(mem::align_of::<T>() <= mem::align_of::<CachePadded<T>>());
}

/// Pad `T` to the length of a cacheline.
///
/// Sometimes concurrent programming requires a piece of data to be padded out to the size of a
/// cacheline to avoid "false sharing": cachelines being invalidated due to unrelated concurrent
/// activity. Use the `CachePadded` type when you want to *avoid* cache locality.
///
/// # Warning
///
/// - The wrapped data is never dropped; move out using `ptr::read` if you need to run dtors.
/// - Do not rely on this actually being padded to the cache line for the correctness of your
///   program. Certain compiler options or hardware might actually cause it to not be aligned.
pub struct CachePadded<T> {
    /// The data.
    ///
    /// This is untyped on purpose in order to force a certain memory representation.
    data: UnsafeCell<[u8; CACHE_LINE]>,
    /// The memory padding.
    _pad: Pad,
    /// The typed marker.
    _marker: marker::PhantomData<T>,
}

impl<T: ZerosValid> CachePadded<T> {
    /// Create a zeroed cache padded value.
    #[cfg(feature = "nightly")]
    pub const fn zeroed() -> CachePadded<T> {
        CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _pad: [],
            _marker: marker::PhantomData,
        }
    }
}

impl<T: ZerosValid> Default for CachePadded<T> {
    fn default() -> CachePadded<T> {
        CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _pad: [],
            _marker: marker::PhantomData,
        }
    }
}

impl<T> CachePadded<T> {
    /// Wrap a value in a type padding it to a cache line.
    ///
    /// # Warning
    ///
    /// The wrapped data is never dropped; move out using `ptr:read` if you need to run dtors.
    ///
    /// # Panics
    ///
    /// If `T` is incompatible with `CachePadded<T>` (e.g. too big), this will hit an assertion.
    pub fn new(t: T) -> CachePadded<T> {
        // Assert the validity.
        assert_valid::<T>();

        // Construct the (zeroed) type.
        let ret = CachePadded {
            data: UnsafeCell::new(([0; CACHE_LINE])),
            _marker: ([], marker::PhantomData),
        };

        // Copy the data into the untyped buffer.
        unsafe {
            let p: *mut T = mem::transmute(&ret.data);
            ptr::write(p, t);
        }

        ret
    }
}

impl<T> fmt::Debug for CachePadded<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "CachePadded {{ ... }}")
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
        assert_valid::<T>();
        let p: *mut T = mem::transmute(&self.data);
        mem::drop(ptr::read(p));
    }
}
*/

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

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
