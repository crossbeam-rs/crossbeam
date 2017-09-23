use std::borrow::{Borrow, BorrowMut};
use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use mutator::Scope;

/// Given ordering for the success case in a compare-exchange operation, returns the strongest
/// appropriate ordering for the failure case.
#[inline]
fn strongest_failure_ordering(ord: Ordering) -> Ordering {
    use self::Ordering::*;
    match ord {
        Relaxed | Release => Relaxed,
        Acquire | AcqRel => Acquire,
        _ => SeqCst,
    }
}

/// Memory orderings for compare-and-set operations.
///
/// A compare-and-set operation can have different memory orderings depending on whether it
/// succeeds or fails. This trait generalizes different ways of specifying memory orderings.
///
/// The two ways of specifying orderings for compare-and-set are:
///
/// 1. Just one `Ordering` for the success case. In case of failure, the strongest appropriate
///    ordering is chosen.
/// 2. A pair of `Ordering`s. The first one is for the success case, while the second one is
///    for the failure case.
pub trait CompareAndSetOrdering {
    /// The ordering of the operation when it succeeds.
    fn success(&self) -> Ordering;

    /// The ordering of the operation when it fails.
    ///
    /// The failure ordering can't be `Release` or `AcqRel` and must be equivalent or weaker than
    /// the success ordering.
    fn failure(&self) -> Ordering;
}

impl CompareAndSetOrdering for Ordering {
    #[inline]
    fn success(&self) -> Ordering {
        *self
    }

    #[inline]
    fn failure(&self) -> Ordering {
        strongest_failure_ordering(*self)
    }
}

impl CompareAndSetOrdering for (Ordering, Ordering) {
    #[inline]
    fn success(&self) -> Ordering {
        self.0
    }

    #[inline]
    fn failure(&self) -> Ordering {
        self.1
    }
}

/// Panics if the pointer is not properly unaligned.
#[inline]
fn ensure_aligned<T>(raw: *const T) {
    assert_eq!(raw as usize & low_bits::<T>(), 0, "unaligned pointer");
}

/// Returns a bitmask containing the unused least significant bits of an aligned pointer to `T`.
#[inline]
fn low_bits<T>() -> usize {
    (1 << mem::align_of::<T>().trailing_zeros()) - 1
}

/// Given a tagged pointer `data`, returns the same pointer, but tagged with `tag`.  `tag` is
/// truncated to be fit into the unused bits of the pointer to `T`.
#[inline]
fn data_with_tag<T>(data: usize, tag: usize) -> usize {
    (data & !low_bits::<T>()) | (tag & low_bits::<T>())
}

/// An atomic pointer that can be safely shared between threads.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.  More precisely, a tag should be less than `(1 <<
/// mem::align_of::<T>().trailing_zeros())`.
///
/// Any method that loads the pointer must be passed a reference to a [`Scope`].
///
/// [`Scope`]: struct.Scope.html
#[derive(Debug)]
pub struct Atomic<T> {
    data: AtomicUsize,
    _marker: PhantomData<*mut T>,
}

unsafe impl<T: Send + Sync> Send for Atomic<T> {}
unsafe impl<T: Send + Sync> Sync for Atomic<T> {}

impl<T> Atomic<T> {
    /// Returns a new atomic pointer pointing to the tagged pointer `data`.
    fn from_data(data: usize) -> Self {
        Atomic {
            data: AtomicUsize::new(data),
            _marker: PhantomData,
        }
    }

    /// Returns a new null atomic pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Atomic;
    ///
    /// let a = Atomic::<i32>::null();
    /// ```
    #[cfg(not(feature = "nightly"))]
    pub fn null() -> Self {
        Atomic {
            data: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Returns a new null atomic pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Atomic;
    ///
    /// let a = Atomic::<i32>::null();
    /// ```
    #[cfg(feature = "nightly")]
    pub const fn null() -> Self {
        Atomic {
            data: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

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
        Self::from_owned(Owned::new(value))
    }

    /// Returns a new atomic pointer pointing to `owned`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{Atomic, Owned};
    ///
    /// let a = Atomic::from_owned(Owned::new(1234));
    /// ```
    pub fn from_owned(owned: Owned<T>) -> Self {
        let data = owned.data;
        mem::forget(owned);
        Self::from_data(data)
    }

    /// Returns a new atomic pointer pointing to `ptr`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{Atomic, Ptr};
    ///
    /// let a = Atomic::from_ptr(Ptr::<i32>::null());
    /// ```
    pub fn from_ptr(ptr: Ptr<T>) -> Self {
        Self::from_data(ptr.data)
    }

    /// Loads a `Ptr` from the atomic pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// epoch::pin(|scope| {
    ///     let p = a.load(SeqCst, scope);
    /// });
    /// ```
    pub fn load<'scope>(&self, ord: Ordering, _: &'scope Scope) -> Ptr<'scope, T> {
        Ptr::from_data(self.data.load(ord))
    }

    /// Stores a `Ptr` into the atomic pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// a.store(Ptr::null(), SeqCst);
    /// ```
    pub fn store(&self, new: Ptr<T>, ord: Ordering) {
        self.data.store(new.data, ord);
    }

    /// Stores an `Owned` into the atomic pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::null();
    /// a.store_owned(Owned::new(1234), SeqCst);
    /// ```
    pub fn store_owned(&self, new: Owned<T>, ord: Ordering) {
        let data = new.data;
        mem::forget(new);
        self.data.store(data, ord);
    }

    /// Stores a `Ptr` into the atomic pointer, returning the previous `Ptr`.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// epoch::pin(|scope| {
    ///     let p = a.swap(Ptr::null(), SeqCst, scope);
    /// });
    /// ```
    pub fn swap<'scope>(&self, new: Ptr<T>, ord: Ordering, _: &'scope Scope) -> Ptr<'scope, T> {
        Ptr::from_data(self.data.swap(new.data, ord))
    }

    /// Stores `new` into the atomic pointer if the current value is the same as `current`.
    ///
    /// The return value is a result indicating whether the new pointer was written. On failure the
    /// actual current value is returned.
    ///
    /// This method takes a [`CompareAndSetOrdering`] argument which describes the memory
    /// ordering of this operation.
    ///
    /// [`CompareAndSetOrdering`]: trait.CompareAndSetOrdering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    ///
    /// epoch::pin(|scope| {
    ///     let mut curr = a.load(SeqCst, scope);
    ///     let res = a.compare_and_set(curr, Ptr::null(), SeqCst, scope);
    /// });
    /// ```
    pub fn compare_and_set<'scope, O>(
        &self,
        current: Ptr<T>,
        new: Ptr<T>,
        ord: O,
        _: &'scope Scope,
    ) -> Result<(), Ptr<'scope, T>>
    where
        O: CompareAndSetOrdering,
    {
        match self.data.compare_exchange(
            current.data,
            new.data,
            ord.success(),
            ord.failure(),
        ) {
            Ok(_) => Ok(()),
            Err(previous) => Err(Ptr::from_data(previous)),
        }
    }

    /// Stores `new` into the atomic pointer if the current value is the same as `current`.
    ///
    /// Unlike [`compare_and_set`], this method is allowed to spuriously fail even when
    /// comparison succeeds, which can result in more efficient code on some platforms.
    /// The return value is a result indicating whether the new pointer was written. On failure the
    /// actual current value is returned.
    ///
    /// This method takes a [`CompareAndSetOrdering`] argument which describes the memory
    /// ordering of this operation.
    ///
    /// [`compare_and_set`]: struct.Atomic.html#method.compare_and_set
    /// [`CompareAndSetOrdering`]: trait.CompareAndSetOrdering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    ///
    /// epoch::pin(|scope| {
    ///     let mut curr = a.load(SeqCst, scope);
    ///     loop {
    ///         match a.compare_and_set(curr, Ptr::null(), SeqCst, scope) {
    ///             Ok(()) => break,
    ///             Err(c) => curr = c,
    ///         }
    ///     }
    /// });
    /// ```
    pub fn compare_and_set_weak<'scope, O>(
        &self,
        current: Ptr<T>,
        new: Ptr<T>,
        ord: O,
        _: &'scope Scope,
    ) -> Result<(), Ptr<'scope, T>>
    where
        O: CompareAndSetOrdering,
    {
        match self.data.compare_exchange_weak(
            current.data,
            new.data,
            ord.success(),
            ord.failure(),
        ) {
            Ok(_) => Ok(()),
            Err(previous) => Err(Ptr::from_data(previous)),
        }
    }

    /// Stores `new` into the atomic pointer if the current value is the same as `current`.
    ///
    /// The return value is a result indicating whether the new pointer was written. On success the
    /// pointer that was written is returned. On failure `new` and the actual current value are
    /// returned.
    ///
    /// This method takes a [`CompareAndSetOrdering`] argument which describes the memory
    /// ordering of this operation.
    ///
    /// [`CompareAndSetOrdering`]: trait.CompareAndSetOrdering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    ///
    /// epoch::pin(|scope| {
    ///     let mut curr = a.load(SeqCst, scope);
    ///     let res = a.compare_and_set_owned(curr, Owned::new(5678), SeqCst, scope);
    /// });
    /// ```
    pub fn compare_and_set_owned<'scope, O>(
        &self,
        current: Ptr<T>,
        new: Owned<T>,
        ord: O,
        _: &'scope Scope,
    ) -> Result<Ptr<'scope, T>, (Ptr<'scope, T>, Owned<T>)>
    where
        O: CompareAndSetOrdering,
    {
        match self.data.compare_exchange(
            current.data,
            new.data,
            ord.success(),
            ord.failure(),
        ) {
            Ok(_) => {
                let data = new.data;
                mem::forget(new);
                Ok(Ptr::from_data(data))
            }
            Err(previous) => Err((Ptr::from_data(previous), new)),
        }
    }

    /// Stores `new` into the atomic pointer if the current value is the same as `current`.
    ///
    /// Unlike [`compare_and_set_owned`], this method is allowed to spuriously fail even when
    /// comparison succeeds, which can result in more efficient code on some platforms.
    /// The return value is a result indicating whether the new pointer was written. On success the
    /// pointer that was written is returned. On failure `new` and the actual current value are
    /// returned.
    ///
    /// This method takes a [`CompareAndSetOrdering`] argument which describes the memory
    /// ordering of this operation.
    ///
    /// [`compare_and_set_owned`]: struct.Atomic.html#method.compare_and_set_owned
    /// [`CompareAndSetOrdering`]: trait.CompareAndSetOrdering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    ///
    /// epoch::pin(|scope| {
    ///     let mut new = Owned::new(5678);
    ///     let mut ptr = a.load(SeqCst, scope);
    ///     loop {
    ///         match a.compare_and_set_weak_owned(ptr, new, SeqCst, scope) {
    ///             Ok(p) => {
    ///                 ptr = p;
    ///                 break;
    ///             }
    ///             Err((p, n)) => {
    ///                 ptr = p;
    ///                 new = n;
    ///             }
    ///         }
    ///     }
    /// });
    /// ```
    pub fn compare_and_set_weak_owned<'scope, O>(
        &self,
        current: Ptr<T>,
        new: Owned<T>,
        ord: O,
        _: &'scope Scope,
    ) -> Result<Ptr<'scope, T>, (Ptr<'scope, T>, Owned<T>)>
    where
        O: CompareAndSetOrdering,
    {
        match self.data.compare_exchange_weak(
            current.data,
            new.data,
            ord.success(),
            ord.failure(),
        ) {
            Ok(_) => {
                let data = new.data;
                mem::forget(new);
                Ok(Ptr::from_data(data))
            }
            Err(previous) => Err((Ptr::from_data(previous), new)),
        }
    }

    /// Bitwise "and" with the current tag.
    ///
    /// Performs a bitwise "and" operation on the current tag and the argument `val`, and sets the
    /// new tag to the result. Returns the previous pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<i32>::from_ptr(Ptr::null().with_tag(3));
    /// epoch::pin(|scope| {
    ///     assert_eq!(a.fetch_and(2, SeqCst, scope).tag(), 3);
    ///     assert_eq!(a.load(SeqCst, scope).tag(), 2);
    /// });
    /// ```
    pub fn fetch_and<'scope>(&self, val: usize, ord: Ordering, _: &'scope Scope) -> Ptr<'scope, T> {
        Ptr::from_data(self.data.fetch_and(val | !low_bits::<T>(), ord))
    }

    /// Bitwise "or" with the current tag.
    ///
    /// Performs a bitwise "or" operation on the current tag and the argument `val`, and sets the
    /// new tag to the result. Returns the previous pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<i32>::from_ptr(Ptr::null().with_tag(1));
    /// epoch::pin(|scope| {
    ///     assert_eq!(a.fetch_or(2, SeqCst, scope).tag(), 1);
    ///     assert_eq!(a.load(SeqCst, scope).tag(), 3);
    /// });
    /// ```
    pub fn fetch_or<'scope>(&self, val: usize, ord: Ordering, _: &'scope Scope) -> Ptr<'scope, T> {
        Ptr::from_data(self.data.fetch_or(val & low_bits::<T>(), ord))
    }

    /// Bitwise "xor" with the current tag.
    ///
    /// Performs a bitwise "xor" operation on the current tag and the argument `val`, and sets the
    /// new tag to the result. Returns the previous pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Ptr};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<i32>::from_ptr(Ptr::null().with_tag(1));
    /// epoch::pin(|scope| {
    ///     assert_eq!(a.fetch_xor(3, SeqCst, scope).tag(), 1);
    ///     assert_eq!(a.load(SeqCst, scope).tag(), 2);
    /// });
    /// ```
    pub fn fetch_xor<'scope>(&self, val: usize, ord: Ordering, _: &'scope Scope) -> Ptr<'scope, T> {
        Ptr::from_data(self.data.fetch_xor(val & low_bits::<T>(), ord))
    }
}

impl<T> Default for Atomic<T> {
    fn default() -> Self {
        Atomic::null()
    }
}

impl<T> From<T> for Atomic<T> {
    fn from(t: T) -> Self {
        Atomic::new(t)
    }
}

impl<T> From<Box<T>> for Atomic<T> {
    fn from(b: Box<T>) -> Self {
        Atomic::from_owned(Owned::from_box(b))
    }
}

impl<T> From<Owned<T>> for Atomic<T> {
    fn from(owned: Owned<T>) -> Self {
        Atomic::from_owned(owned)
    }
}

impl<'scope, T> From<Ptr<'scope, T>> for Atomic<T> {
    fn from(ptr: Ptr<T>) -> Self {
        Atomic::from_ptr(ptr)
    }
}

/// An owned heap-allocated object.
///
/// This type is very similar to `Box<T>`.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.
#[derive(Debug)]
pub struct Owned<T> {
    data: usize,
    _marker: PhantomData<Box<T>>,
}

impl<T> Owned<T> {
    /// Returns a new owned pointer pointing to the tagged pointer `data`.
    unsafe fn from_data(data: usize) -> Self {
        Owned {
            data: data,
            _marker: PhantomData,
        }
    }

    /// Allocates `value` on the heap and returns a new owned pointer pointing to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = Owned::new(1234);
    /// ```
    pub fn new(value: T) -> Self {
        Self::from_box(Box::new(value))
    }

    /// Returns a new owned pointer pointing to `b`.
    ///
    /// # Panics
    ///
    /// Panics if the pointer (the `Box`) is not properly aligned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = unsafe { Owned::from_raw(Box::into_raw(Box::new(1234))) };
    /// ```
    pub fn from_box(b: Box<T>) -> Self {
        unsafe { Self::from_raw(Box::into_raw(b)) }
    }

    /// Returns a new owned pointer pointing to `raw`.
    ///
    /// This function is unsafe because improper use may lead to memory problems. Argument `raw`
    /// must be a valid pointer. Also, a double-free may occur if the function is called twice on
    /// the same raw pointer.
    ///
    /// # Panics
    ///
    /// Panics if `raw` is not properly aligned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = unsafe { Owned::from_raw(Box::into_raw(Box::new(1234))) };
    /// ```
    pub unsafe fn from_raw(raw: *mut T) -> Self {
        ensure_aligned(raw);
        Self::from_data(raw as usize)
    }

    /// Converts the owned pointer to a [`Ptr`].
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Owned};
    ///
    /// let o = Owned::new(1234);
    /// epoch::pin(|scope| {
    ///     let p = o.into_ptr(scope);
    /// });
    /// ```
    ///
    /// [`Ptr`]: struct.Ptr.html
    pub fn into_ptr<'scope>(self, _: &'scope Scope) -> Ptr<'scope, T> {
        let data = self.data;
        mem::forget(self);
        Ptr::from_data(data)
    }

    /// Returns the tag stored within the pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// assert_eq!(Owned::new(1234).tag(), 0);
    /// ```
    pub fn tag(&self) -> usize {
        self.data & low_bits::<T>()
    }

    /// Returns the same pointer, but tagged with `tag`. `tag` is truncated to be fit into the
    /// unused bits of the pointer to `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = Owned::new(0u64);
    /// assert_eq!(o.tag(), 0);
    /// let o = o.with_tag(5);
    /// assert_eq!(o.tag(), 5);
    /// ```
    pub fn with_tag(self, tag: usize) -> Self {
        let data = self.data;
        mem::forget(self);
        unsafe { Self::from_data(data_with_tag::<T>(data, tag)) }
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        let raw = (self.data & !low_bits::<T>()) as *mut T;
        unsafe {
            drop(Box::from_raw(raw));
        }
    }
}

impl<T> Deref for Owned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*((self.data & !low_bits::<T>()) as *const T) }
    }
}

impl<T> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *((self.data & !low_bits::<T>()) as *mut T) }
    }
}

impl<T> From<T> for Owned<T> {
    fn from(t: T) -> Self {
        Owned::new(t)
    }
}

impl<T> From<Box<T>> for Owned<T> {
    fn from(b: Box<T>) -> Self {
        Owned::from_box(b)
    }
}

impl<T> Borrow<T> for Owned<T> {
    fn borrow(&self) -> &T {
        &**self
    }
}

impl<T> BorrowMut<T> for Owned<T> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T> AsRef<T> for Owned<T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

impl<T> AsMut<T> for Owned<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

/// A pointer to an object protected by the epoch GC.
///
/// The pointer is valid for use only within `'scope`.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.
#[derive(Debug)]
pub struct Ptr<'scope, T: 'scope> {
    data: usize,
    _marker: PhantomData<(&'scope (), *const T)>,
}

unsafe impl<'scope, T: Send> Send for Ptr<'scope, T> {}

impl<'scope, T> Clone for Ptr<'scope, T> {
    fn clone(&self) -> Self {
        Ptr {
            data: self.data,
            _marker: PhantomData,
        }
    }
}

impl<'scope, T> Copy for Ptr<'scope, T> {}

impl<'scope, T> Ptr<'scope, T> {
    /// Returns a new pointer pointing to the tagged pointer `data`.
    fn from_data(data: usize) -> Self {
        Ptr {
            data: data,
            _marker: PhantomData,
        }
    }

    /// Returns a new null pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Ptr;
    ///
    /// let p = Ptr::<i32>::null();
    /// assert!(p.is_null());
    /// ```
    pub fn null() -> Self {
        Ptr {
            data: 0,
            _marker: PhantomData,
        }
    }

    /// Returns a new pointer pointing to `raw`.
    ///
    /// # Panics
    ///
    /// Panics if `raw` is not properly aligned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Ptr;
    ///
    /// let p = unsafe { Ptr::from_raw(Box::into_raw(Box::new(1234))) };
    /// assert!(!p.is_null());
    /// ```
    pub fn from_raw(raw: *const T) -> Self {
        ensure_aligned(raw);
        Ptr {
            data: raw as usize,
            _marker: PhantomData,
        }
    }

    /// Returns `true` if the pointer is null.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::null();
    /// epoch::pin(|scope| {
    ///     assert!(a.load(SeqCst, scope).is_null());
    ///     a.store_owned(Owned::new(1234), SeqCst);
    ///     assert!(!a.load(SeqCst, scope).is_null());
    /// });
    /// ```
    pub fn is_null(&self) -> bool {
        self.as_raw().is_null()
    }

    /// Converts the pointer to a raw pointer (without the tag).
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let o = Owned::new(1234);
    /// let raw = &*o as *const _;
    /// let a = Atomic::from_owned(o);
    ///
    /// epoch::pin(|scope| {
    ///     let p = a.load(SeqCst, scope);
    ///     assert_eq!(p.as_raw(), raw);
    /// });
    /// ```
    pub fn as_raw(&self) -> *const T {
        (self.data & !low_bits::<T>()) as *const T
    }

    /// Dereferences the pointer.
    ///
    /// Returns a reference to the pointee that is valid in `'scope`.
    ///
    /// # Safety
    ///
    /// Dereferencing a pointer is unsafe because it could be pointing to invalid memory.
    ///
    /// Another concern is the possiblity of data races due to lack of proper synchronization.
    /// For example, consider the following scenario:
    ///
    /// 1. A thread creates a new object: `a.store_owned(Owned::new(10), Relaxed)`
    /// 2. Another thread reads it: `*a.load(Relaxed, scope).as_ref().unwrap()`
    ///
    /// The problem is that relaxed orderings don't synchronize initialization of the object with
    /// the read from the second thread. This is a data race. A possible solution would be to use
    /// `Release` and `Acquire` orderings.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// epoch::pin(|scope| {
    ///     let p = a.load(SeqCst, scope);
    ///     unsafe {
    ///         assert_eq!(p.deref(), &1234);
    ///     }
    /// });
    /// ```
    pub unsafe fn deref(&self) -> &'scope T {
        &*self.as_raw()
    }

    /// Converts the pointer to a reference.
    ///
    /// Returns `None` if the pointer is null, or else a reference to the object wrapped in `Some`.
    ///
    /// # Safety
    ///
    /// Dereferencing a pointer is unsafe because it could be pointing to invalid memory.
    ///
    /// Another concern is the possiblity of data races due to lack of proper synchronization.
    /// For example, consider the following scenario:
    ///
    /// 1. A thread creates a new object: `a.store_owned(Owned::new(10), Relaxed)`
    /// 2. Another thread reads it: `*a.load(Relaxed, scope).as_ref().unwrap()`
    ///
    /// The problem is that relaxed orderings don't synchronize initialization of the object with
    /// the read from the second thread. This is a data race. A possible solution would be to use
    /// `Release` and `Acquire` orderings.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// epoch::pin(|scope| {
    ///     let p = a.load(SeqCst, scope);
    ///     unsafe {
    ///         assert_eq!(p.as_ref(), Some(&1234));
    ///     }
    /// });
    /// ```
    pub unsafe fn as_ref(&self) -> Option<&'scope T> {
        self.as_raw().as_ref()
    }

    /// Takes ownership of the pointee.
    ///
    /// # Safety
    ///
    /// This method may be called only if the pointer is valid and nobody else is holding a
    /// reference to the same object.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// unsafe {
    ///     epoch::unprotected(|scope| {
    ///         let p = a.load(SeqCst, scope);
    ///         drop(p.into_owned());
    ///     });
    /// }
    /// ```
    pub unsafe fn into_owned(self) -> Owned<T> {
        Owned::from_data(self.data)
    }

    /// Returns the tag stored within the pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::from_owned(Owned::new(0u64).with_tag(5));
    /// epoch::pin(|scope| {
    ///     let p = a.load(SeqCst, scope);
    ///     assert_eq!(p.tag(), 5);
    /// });
    /// ```
    pub fn tag(&self) -> usize {
        self.data & low_bits::<T>()
    }

    /// Returns the same pointer, but tagged with `tag`. `tag` is truncated to be fit into the
    /// unused bits of the pointer to `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(0u64);
    /// epoch::pin(|scope| {
    ///     let p1 = a.load(SeqCst, scope);
    ///     let p2 = p1.with_tag(5);
    ///
    ///     assert_eq!(p1.tag(), 0);
    ///     assert_eq!(p2.tag(), 5);
    ///     assert_eq!(p1.as_raw(), p2.as_raw());
    /// });
    /// ```
    pub fn with_tag(&self, tag: usize) -> Self {
        Self::from_data(data_with_tag::<T>(self.data, tag))
    }
}

impl<'scope, T> Default for Ptr<'scope, T> {
    fn default() -> Self {
        Ptr::null()
    }
}

#[cfg(test)]
mod tests {
    use super::Ptr;

    #[test]
    fn valid_tag_i8() {
        Ptr::<i8>::null().with_tag(0);
    }

    #[test]
    fn valid_tag_i64() {
        Ptr::<i64>::null().with_tag(7);
    }
}
