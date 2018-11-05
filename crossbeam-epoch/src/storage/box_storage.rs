use alloc::boxed::Box;

use atomic::decompose_data;

use {AtomicTmpl, OwnedTmpl, Pointer, SharedTmpl, Storage};

/// An atomic pointer that can be safely shared between threads.
///
/// See [`AtomicTmpl`] for more details.
///
/// [`AtomicTmpl`]: struct.AtomicTmpl.html
pub type Atomic<T> = AtomicTmpl<T, Box<T>>;

/// An owned heap-allocated object.
///
/// This type is very similar to `Box<T>`.  See [`OwnedTmpl`] for more details.
///
/// [`OwnedTmpl`]: struct.OwnedTmpl.html
pub type Owned<T> = OwnedTmpl<T, Box<T>>;

/// A pointer to an object protected by the epoch GC.
///
/// See [`SharedTmpl`] for more details.
///
/// [`SharedTmpl`]: struct.SharedTmpl.html
pub type Shared<'g, T> = SharedTmpl<'g, T, Box<T>>;

unsafe impl<T> Storage<T> for Box<T> {
    fn into_raw(self) -> *mut T {
        Self::into_raw(self)
    }

    unsafe fn from_raw(data: *mut T) -> Self {
        Self::from_raw(data)
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

    /// Converts the owned pointer into a `Box`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Owned};
    ///
    /// let o = Owned::new(1234);
    /// let b: Box<i32> = o.into_box();
    /// assert_eq!(*b, 1234);
    /// ```
    pub fn into_box(self) -> Box<T> {
        let (raw, _) = decompose_data::<T>(self.into_usize());
        unsafe { Box::from_raw(raw) }
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
