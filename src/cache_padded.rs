use core::fmt;
use core::ops::{Deref, DerefMut};

/// Pads and aligns a value to the length of a cache line.
///
/// In concurrent programming, sometimes it is desirable to make sure commonly accessed pieces of
/// data are not placed into the same cache line. Updating an atomic value invalides the whole
/// cache line it belongs to, which makes the next access to the same cache line slower for other
/// CPU cores. Use `CachePadded` to ensure updating one piece of data doesn't invalidate other
/// cached data.
///
/// Cache lines are assumed to be 64 bytes on all architectures.
///
/// # Size and alignment
///
/// The size of `CachePadded<T>` is the smallest multiple of 64 bytes large enough to accommodate
/// a value of type `T`.
///
/// The alignment of `CachePadded<T>` is the maximum of 64 bytes and the alignment of `T`.
///
/// # Examples
///
/// Alignment and padding:
///
/// ```
/// use crossbeam_utils::CachePadded;
///
/// let array = [CachePadded::new(1i32), CachePadded::new(2i32)];
/// let addr1 = &*array[0] as *const i32 as usize;
/// let addr2 = &*array[1] as *const i32 as usize;
///
/// assert_eq!(addr2 - addr1, 64);
/// assert_eq!(addr1 % 64, 0);
/// assert_eq!(addr2 % 64, 0);
/// ```
///
/// When building a concurrent queue with a head and a tail index, it is wise to place them in
/// different cache lines so that concurrent threads pushing and popping elements don't invalidate
/// each other's cache lines:
///
/// ```
/// use crossbeam_utils::CachePadded;
/// use std::sync::atomic::AtomicUsize;
///
/// struct Queue<T> {
///     head: CachePadded<AtomicUsize>,
///     tail: CachePadded<AtomicUsize>,
///     buffer: *mut T,
/// }
/// ```
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq)]
#[repr(align(64))]
pub struct CachePadded<T> {
    value: T,
}

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

impl<T> CachePadded<T> {
    /// Pads and aligns a value to the length of a cache line.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::CachePadded;
    ///
    /// let padded_value = CachePadded::new(1);
    /// ```
    pub fn new(t: T) -> CachePadded<T> {
        CachePadded::<T> { value: t }
    }

    /// Returns the value value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::CachePadded;
    ///
    /// let padded_value = CachePadded::new(7);
    /// let value = padded_value.into_inner();
    /// assert_eq!(value, 7);
    /// ```
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: fmt::Debug> fmt::Debug for CachePadded<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CachePadded")
            .field("value", &self.value)
            .finish()
    }
}

impl<T> From<T> for CachePadded<T> {
    fn from(t: T) -> Self {
        CachePadded::new(t)
    }
}
