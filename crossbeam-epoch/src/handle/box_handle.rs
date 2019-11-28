use core::mem;

use atomic::{self, Handle};

/// An atomic pointer that can be safely shared between threads.
///
/// See [`Atomic`] for more details.
///
/// [`Atomic`]: atomic/struct.Atomic.html
pub type Atomic<T> = atomic::Atomic<Box<T>>;

/// An owned heap-allocated object.
///
/// This type is very similar to `Box<T>`.  See [`Owned`] for more details.
///
/// [`Owned`]: atomic/struct.Owned.html
pub type Owned<T> = atomic::Owned<Box<T>>;

/// A pointer to an object protected by the epoch GC.
///
/// See [`Shared`] for more details.
///
/// [`Shared`]: atomic/struct.Shared.html
pub type Shared<'g, T> = atomic::Shared<'g, Box<T>>;

unsafe impl<T> Handle for Box<T> {
    const ALIGN: usize = mem::align_of::<T>();

    fn into_usize(self) -> usize {
        Self::into_raw(self) as usize
    }

    unsafe fn from_usize(data: usize) -> Self {
        Self::from_raw(data as *mut _)
    }
}

impl<T> Atomic<T> {
    /// Allocates `value` on the heap and returns a new atomic pointer pointing to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Atomic;
    ///
    /// let a = Atomic::new(1234);
    /// ```
    pub fn new(value: T) -> Self {
        Self::from(Owned::new(value))
    }
}

impl<T> From<T> for Atomic<T> {
    fn from(t: T) -> Self {
        Self::new(t)
    }
}

impl<T> Owned<T> {
    /// Allocates `value` on the heap and returns a new owned pointer pointing to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = Owned::new(1234);
    /// ```
    pub fn new(value: T) -> Owned<T> {
        Self::from(Box::new(value))
    }
}

impl<T: Clone> Clone for Owned<T> {
    fn clone(&self) -> Self {
        Owned::new((**self).clone()).with_tag(self.tag())
    }
}

impl<T> From<T> for Owned<T> {
    fn from(t: T) -> Self {
        Owned::new(t)
    }
}
