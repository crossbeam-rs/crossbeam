use core::fmt;
use core::ops::{Deref, DerefMut};

/// Pads `T` to the length of a cache line.
///
/// Sometimes concurrent programming requires a piece of data to be padded out to the size of a
/// cacheline to avoid "false sharing": cache lines being invalidated due to unrelated concurrent
/// activity. Use this type when you want to *avoid* cache locality.
///
/// Cache lines are assumed to be 64 bytes on all architectures.
///
/// # Size and alignment
///
/// The size of `CachePadded<T>` is the smallest multiple of 64 bytes large enough to accommodate
/// a value of type `T`.
///
/// The alignment of `CachePadded<T>` is the maximum of 64 bytes and the alignment of `T`.
#[derive(Clone, Default)]
#[repr(align(64))]
pub struct CachePadded<T> {
    inner: T,
}

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

impl<T> CachePadded<T> {
    /// Pads a value to the length of a cache line.
    pub fn new(t: T) -> CachePadded<T> {
        CachePadded::<T> { inner: t }
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: fmt::Debug> fmt::Debug for CachePadded<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let inner: &T = &*self;
        write!(f, "CachePadded {{ {:?} }}", inner)
    }
}

impl<T> From<T> for CachePadded<T> {
    fn from(t: T) -> Self {
        CachePadded::new(t)
    }
}
