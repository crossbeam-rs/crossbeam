use alloc::boxed::Box;

use {AtomicTmpl, OwnedTmpl, SharedTmpl, Storage};

/// An atomic pointer that can be safely shared between threads.
///
/// See [`AtomicTmpl`] for more details.
///
/// [`AtomicTmpl`]: struct.AtomicTmpl.html
pub type Atomic<T> = AtomicTmpl<Box<T>>;

/// An owned heap-allocated object.
///
/// This type is very similar to `Box<T>`.  See [`OwnedTmpl`] for more details.
///
/// [`OwnedTmpl`]: struct.OwnedTmpl.html
pub type Owned<T> = OwnedTmpl<Box<T>>;

/// A pointer to an object protected by the epoch GC.
///
/// See [`SharedTmpl`] for more details.
///
/// [`SharedTmpl`]: struct.SharedTmpl.html
pub type Shared<'g, T> = SharedTmpl<'g, Box<T>>;

unsafe impl<T> Storage for Box<T> {
    const ALIGN_OF: usize = ::core::mem::align_of::<T>();

    fn into_raw(self) -> usize {
        Self::into_raw(self) as usize
    }

    unsafe fn from_raw(data: usize) -> Self {
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
