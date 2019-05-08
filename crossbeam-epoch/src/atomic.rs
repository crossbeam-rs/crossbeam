use alloc::boxed::Box;
use core::borrow::{Borrow, BorrowMut};
use core::cmp;
use core::fmt;
use core::marker::PhantomData;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_utils::atomic::AtomicConsume;
use guard::Guard;

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

/// The error returned on failed compare-and-set operation.
pub struct CompareAndSetError<'g, T: 'g, P: Pointer<T>> {
    /// The value in the atomic pointer at the time of the failed operation.
    pub current: Shared<'g, T>,

    /// The new value, which the operation failed to store.
    pub new: P,
}

impl<'g, T: 'g, P: Pointer<T> + fmt::Debug> fmt::Debug for CompareAndSetError<'g, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CompareAndSetError")
            .field("current", &self.current)
            .field("new", &self.new)
            .finish()
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

/// Given a tagged pointer `data`, returns the same pointer, but tagged with `tag`.
///
/// `tag` is truncated to fit into the unused bits of the pointer to `T`.
#[inline]
fn data_with_tag<T>(data: usize, tag: usize) -> usize {
    (data & !low_bits::<T>()) | (tag & low_bits::<T>())
}

/// Decomposes a tagged pointer `data` into the pointer and the tag.
#[inline]
fn decompose_data<T>(data: usize) -> (*mut T, usize) {
    let raw = (data & !low_bits::<T>()) as *mut T;
    let tag = data & low_bits::<T>();
    (raw, tag)
}

/// An atomic pointer that can be safely shared between threads.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.  More precisely, a tag should be less than `(1 <<
/// mem::align_of::<T>().trailing_zeros())`.
///
/// Any method that loads the pointer must be passed a reference to a [`Guard`].
///
/// [`Guard`]: struct.Guard.html
pub struct Atomic<T> {
    data: AtomicUsize,
    _marker: PhantomData<*mut T>,
}

unsafe impl<T: Send + Sync> Send for Atomic<T> {}
unsafe impl<T: Send + Sync> Sync for Atomic<T> {}

impl<T> Atomic<T> {
    /// Returns a new atomic pointer pointing to the tagged pointer `data`.
    fn from_usize(data: usize) -> Self {
        Self {
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
    pub fn null() -> Atomic<T> {
        Self {
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
    pub const fn null() -> Atomic<T> {
        Self {
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
    pub fn new(value: T) -> Atomic<T> {
        Self::from(Owned::new(value))
    }

    /// Loads a `Shared` from the atomic pointer.
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
    /// let guard = &epoch::pin();
    /// let p = a.load(SeqCst, guard);
    /// ```
    pub fn load<'g>(&self, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.load(ord)) }
    }

    /// Loads a `Shared` from the atomic pointer using a "consume" memory ordering.
    ///
    /// This is similar to the "acquire" ordering, except that an ordering is
    /// only guaranteed with operations that "depend on" the result of the load.
    /// However consume loads are usually much faster than acquire loads on
    /// architectures with a weak memory model since they don't require memory
    /// fence instructions.
    ///
    /// The exact definition of "depend on" is a bit vague, but it works as you
    /// would expect in practice since a lot of software, especially the Linux
    /// kernel, rely on this behavior.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    ///
    /// let a = Atomic::new(1234);
    /// let guard = &epoch::pin();
    /// let p = a.load_consume(guard);
    /// ```
    pub fn load_consume<'g>(&self, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.load_consume()) }
    }

    /// Stores a `Shared` or `Owned` pointer into the atomic pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// a.store(Shared::null(), SeqCst);
    /// a.store(Owned::new(1234), SeqCst);
    /// ```
    pub fn store<'g, P: Pointer<T>>(&self, new: P, ord: Ordering) {
        self.data.store(new.into_usize(), ord);
    }

    /// Stores a `Shared` or `Owned` pointer into the atomic pointer, returning the previous
    /// `Shared`.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// [`Ordering`]: https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// let guard = &epoch::pin();
    /// let p = a.swap(Shared::null(), SeqCst, guard);
    /// ```
    pub fn swap<'g, P: Pointer<T>>(&self, new: P, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.swap(new.into_usize(), ord)) }
    }

    /// Stores the pointer `new` (either `Shared` or `Owned`) into the atomic pointer if the current
    /// value is the same as `current`. The tag is also taken into account, so two pointers to the
    /// same object, but with different tags, will not be considered equal.
    ///
    /// The return value is a result indicating whether the new pointer was written. On success the
    /// pointer that was written is returned. On failure the actual current value and `new` are
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
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    ///
    /// let guard = &epoch::pin();
    /// let mut curr = a.load(SeqCst, guard);
    /// let res1 = a.compare_and_set(curr, Shared::null(), SeqCst, guard);
    /// let res2 = a.compare_and_set(curr, Owned::new(5678), SeqCst, guard);
    /// ```
    pub fn compare_and_set<'g, O, P>(
        &self,
        current: Shared<T>,
        new: P,
        ord: O,
        _: &'g Guard,
    ) -> Result<Shared<'g, T>, CompareAndSetError<'g, T, P>>
    where
        O: CompareAndSetOrdering,
        P: Pointer<T>,
    {
        let new = new.into_usize();
        self.data
            .compare_exchange(current.into_usize(), new, ord.success(), ord.failure())
            .map(|_| unsafe { Shared::from_usize(new) })
            .map_err(|current| unsafe {
                CompareAndSetError {
                    current: Shared::from_usize(current),
                    new: P::from_usize(new),
                }
            })
    }

    /// Stores the pointer `new` (either `Shared` or `Owned`) into the atomic pointer if the current
    /// value is the same as `current`. The tag is also taken into account, so two pointers to the
    /// same object, but with different tags, will not be considered equal.
    ///
    /// Unlike [`compare_and_set`], this method is allowed to spuriously fail even when comparison
    /// succeeds, which can result in more efficient code on some platforms.  The return value is a
    /// result indicating whether the new pointer was written. On success the pointer that was
    /// written is returned. On failure the actual current value and `new` are returned.
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
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// let guard = &epoch::pin();
    ///
    /// let mut new = Owned::new(5678);
    /// let mut ptr = a.load(SeqCst, guard);
    /// loop {
    ///     match a.compare_and_set_weak(ptr, new, SeqCst, guard) {
    ///         Ok(p) => {
    ///             ptr = p;
    ///             break;
    ///         }
    ///         Err(err) => {
    ///             ptr = err.current;
    ///             new = err.new;
    ///         }
    ///     }
    /// }
    ///
    /// let mut curr = a.load(SeqCst, guard);
    /// loop {
    ///     match a.compare_and_set_weak(curr, Shared::null(), SeqCst, guard) {
    ///         Ok(_) => break,
    ///         Err(err) => curr = err.current,
    ///     }
    /// }
    /// ```
    pub fn compare_and_set_weak<'g, O, P>(
        &self,
        current: Shared<T>,
        new: P,
        ord: O,
        _: &'g Guard,
    ) -> Result<Shared<'g, T>, CompareAndSetError<'g, T, P>>
    where
        O: CompareAndSetOrdering,
        P: Pointer<T>,
    {
        let new = new.into_usize();
        self.data
            .compare_exchange_weak(current.into_usize(), new, ord.success(), ord.failure())
            .map(|_| unsafe { Shared::from_usize(new) })
            .map_err(|current| unsafe {
                CompareAndSetError {
                    current: Shared::from_usize(current),
                    new: P::from_usize(new),
                }
            })
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
    /// use crossbeam_epoch::{self as epoch, Atomic, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<i32>::from(Shared::null().with_tag(3));
    /// let guard = &epoch::pin();
    /// assert_eq!(a.fetch_and(2, SeqCst, guard).tag(), 3);
    /// assert_eq!(a.load(SeqCst, guard).tag(), 2);
    /// ```
    pub fn fetch_and<'g>(&self, val: usize, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.fetch_and(val | !low_bits::<T>(), ord)) }
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
    /// use crossbeam_epoch::{self as epoch, Atomic, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<i32>::from(Shared::null().with_tag(1));
    /// let guard = &epoch::pin();
    /// assert_eq!(a.fetch_or(2, SeqCst, guard).tag(), 1);
    /// assert_eq!(a.load(SeqCst, guard).tag(), 3);
    /// ```
    pub fn fetch_or<'g>(&self, val: usize, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.fetch_or(val & low_bits::<T>(), ord)) }
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
    /// use crossbeam_epoch::{self as epoch, Atomic, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<i32>::from(Shared::null().with_tag(1));
    /// let guard = &epoch::pin();
    /// assert_eq!(a.fetch_xor(3, SeqCst, guard).tag(), 1);
    /// assert_eq!(a.load(SeqCst, guard).tag(), 2);
    /// ```
    pub fn fetch_xor<'g>(&self, val: usize, ord: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.data.fetch_xor(val & low_bits::<T>(), ord)) }
    }

    /// Takes ownership of the pointee.
    ///
    /// This consumes the atomic and converts it into [`Owned`]. As [`Atomic`] doesn't have a
    /// destructor and doesn't drop the pointee while [`Owned`] does, this is suitable for
    /// destructors of data structures.
    ///
    /// # Panics
    ///
    /// Panics if this pointer is null, but only in debug mode.
    ///
    /// # Safety
    ///
    /// This method may be called only if the pointer is valid and nobody else is holding a
    /// reference to the same object.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use std::mem;
    /// # use crossbeam_epoch::Atomic;
    /// struct DataStructure {
    ///     ptr: Atomic<usize>,
    /// }
    ///
    /// impl Drop for DataStructure {
    ///     fn drop(&mut self) {
    ///         // By now the DataStructure lives only in our thread and we are sure we don't hold
    ///         // any Shared or & to it ourselves.
    ///         unsafe {
    ///             drop(mem::replace(&mut self.ptr, Atomic::null()).into_owned());
    ///         }
    ///     }
    /// }
    /// ```
    pub unsafe fn into_owned(self) -> Owned<T> {
        Owned::from_usize(self.data.into_inner())
    }
}

impl<T> fmt::Debug for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data.load(Ordering::SeqCst);
        let (raw, tag) = decompose_data::<T>(data);

        f.debug_struct("Atomic")
            .field("raw", &raw)
            .field("tag", &tag)
            .finish()
    }
}

impl<T> fmt::Pointer for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data = self.data.load(Ordering::SeqCst);
        let (raw, _) = decompose_data::<T>(data);
        fmt::Pointer::fmt(&raw, f)
    }
}

impl<T> Clone for Atomic<T> {
    /// Returns a copy of the atomic value.
    ///
    /// Note that a `Relaxed` load is used here. If you need synchronization, use it with other
    /// atomics or fences.
    fn clone(&self) -> Self {
        let data = self.data.load(Ordering::Relaxed);
        Atomic::from_usize(data)
    }
}

impl<T> Default for Atomic<T> {
    fn default() -> Self {
        Atomic::null()
    }
}

impl<T> From<Owned<T>> for Atomic<T> {
    /// Returns a new atomic pointer pointing to `owned`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{Atomic, Owned};
    ///
    /// let a = Atomic::<i32>::from(Owned::new(1234));
    /// ```
    fn from(owned: Owned<T>) -> Self {
        let data = owned.data;
        mem::forget(owned);
        Self::from_usize(data)
    }
}

impl<T> From<Box<T>> for Atomic<T> {
    fn from(b: Box<T>) -> Self {
        Self::from(Owned::from(b))
    }
}

impl<T> From<T> for Atomic<T> {
    fn from(t: T) -> Self {
        Self::new(t)
    }
}

impl<'g, T> From<Shared<'g, T>> for Atomic<T> {
    /// Returns a new atomic pointer pointing to `ptr`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{Atomic, Shared};
    ///
    /// let a = Atomic::<i32>::from(Shared::<i32>::null());
    /// ```
    fn from(ptr: Shared<'g, T>) -> Self {
        Self::from_usize(ptr.data)
    }
}

impl<T> From<*const T> for Atomic<T> {
    /// Returns a new atomic pointer pointing to `raw`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::ptr;
    /// use crossbeam_epoch::Atomic;
    ///
    /// let a = Atomic::<i32>::from(ptr::null::<i32>());
    /// ```
    fn from(raw: *const T) -> Self {
        Self::from_usize(raw as usize)
    }
}

/// A trait for either `Owned` or `Shared` pointers.
pub trait Pointer<T> {
    /// Returns the machine representation of the pointer.
    fn into_usize(self) -> usize;

    /// Returns a new pointer pointing to the tagged pointer `data`.
    unsafe fn from_usize(data: usize) -> Self;
}

/// An owned heap-allocated object.
///
/// This type is very similar to `Box<T>`.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.
pub struct Owned<T> {
    data: usize,
    _marker: PhantomData<Box<T>>,
}

impl<T> Pointer<T> for Owned<T> {
    #[inline]
    fn into_usize(self) -> usize {
        let data = self.data;
        mem::forget(self);
        data
    }

    /// Returns a new pointer pointing to the tagged pointer `data`.
    ///
    /// # Panics
    ///
    /// Panics if the data is zero in debug mode.
    #[inline]
    unsafe fn from_usize(data: usize) -> Self {
        debug_assert!(data != 0, "converting zero into `Owned`");
        Owned {
            data: data,
            _marker: PhantomData,
        }
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
    pub unsafe fn from_raw(raw: *mut T) -> Owned<T> {
        ensure_aligned(raw);
        Self::from_usize(raw as usize)
    }

    /// Converts the owned pointer into a [`Shared`].
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Owned};
    ///
    /// let o = Owned::new(1234);
    /// let guard = &epoch::pin();
    /// let p = o.into_shared(guard);
    /// ```
    ///
    /// [`Shared`]: struct.Shared.html
    pub fn into_shared<'g>(self, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_usize(self.into_usize()) }
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
        let (raw, _) = decompose_data::<T>(self.data);
        mem::forget(self);
        unsafe { Box::from_raw(raw) }
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
        let (_, tag) = decompose_data::<T>(self.data);
        tag
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
    /// let o = o.with_tag(2);
    /// assert_eq!(o.tag(), 2);
    /// ```
    pub fn with_tag(self, tag: usize) -> Owned<T> {
        let data = self.into_usize();
        unsafe { Self::from_usize(data_with_tag::<T>(data, tag)) }
    }
}

impl<T> Drop for Owned<T> {
    fn drop(&mut self) {
        let (raw, _) = decompose_data::<T>(self.data);
        unsafe {
            drop(Box::from_raw(raw));
        }
    }
}

impl<T> fmt::Debug for Owned<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (raw, tag) = decompose_data::<T>(self.data);

        f.debug_struct("Owned")
            .field("raw", &raw)
            .field("tag", &tag)
            .finish()
    }
}

impl<T: Clone> Clone for Owned<T> {
    fn clone(&self) -> Self {
        Owned::new((**self).clone()).with_tag(self.tag())
    }
}

impl<T> Deref for Owned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        let (raw, _) = decompose_data::<T>(self.data);
        unsafe { &*raw }
    }
}

impl<T> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        let (raw, _) = decompose_data::<T>(self.data);
        unsafe { &mut *raw }
    }
}

impl<T> From<T> for Owned<T> {
    fn from(t: T) -> Self {
        Owned::new(t)
    }
}

impl<T> From<Box<T>> for Owned<T> {
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
    fn from(b: Box<T>) -> Self {
        unsafe { Self::from_raw(Box::into_raw(b)) }
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
/// The pointer is valid for use only during the lifetime `'g`.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.
pub struct Shared<'g, T: 'g> {
    data: usize,
    _marker: PhantomData<(&'g (), *const T)>,
}

impl<'g, T> Clone for Shared<'g, T> {
    fn clone(&self) -> Self {
        Shared {
            data: self.data,
            _marker: PhantomData,
        }
    }
}

impl<'g, T> Copy for Shared<'g, T> {}

impl<'g, T> Pointer<T> for Shared<'g, T> {
    #[inline]
    fn into_usize(self) -> usize {
        self.data
    }

    #[inline]
    unsafe fn from_usize(data: usize) -> Self {
        Shared {
            data: data,
            _marker: PhantomData,
        }
    }
}

impl<'g, T> Shared<'g, T> {
    /// Returns a new null pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Shared;
    ///
    /// let p = Shared::<i32>::null();
    /// assert!(p.is_null());
    /// ```
    pub fn null() -> Shared<'g, T> {
        Shared {
            data: 0,
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
    /// let guard = &epoch::pin();
    /// assert!(a.load(SeqCst, guard).is_null());
    /// a.store(Owned::new(1234), SeqCst);
    /// assert!(!a.load(SeqCst, guard).is_null());
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
    /// let a = Atomic::from(o);
    ///
    /// let guard = &epoch::pin();
    /// let p = a.load(SeqCst, guard);
    /// assert_eq!(p.as_raw(), raw);
    /// ```
    pub fn as_raw(&self) -> *const T {
        let (raw, _) = decompose_data::<T>(self.data);
        raw
    }

    /// Dereferences the pointer.
    ///
    /// Returns a reference to the pointee that is valid during the lifetime `'g`.
    ///
    /// # Safety
    ///
    /// Dereferencing a pointer is unsafe because it could be pointing to invalid memory.
    ///
    /// Another concern is the possiblity of data races due to lack of proper synchronization.
    /// For example, consider the following scenario:
    ///
    /// 1. A thread creates a new object: `a.store(Owned::new(10), Relaxed)`
    /// 2. Another thread reads it: `*a.load(Relaxed, guard).as_ref().unwrap()`
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
    /// let guard = &epoch::pin();
    /// let p = a.load(SeqCst, guard);
    /// unsafe {
    ///     assert_eq!(p.deref(), &1234);
    /// }
    /// ```
    pub unsafe fn deref(&self) -> &'g T {
        &*self.as_raw()
    }

    /// Dereferences the pointer.
    ///
    /// Returns a mutable reference to the pointee that is valid during the lifetime `'g`.
    ///
    /// # Safety
    ///
    /// * There is no guarantee that there are no more threads attempting to read/write from/to the
    ///   actual object at the same time.
    ///
    ///   The user must know that there are no concurrent accesses towards the object itself.
    ///
    /// * Other than the above, all safety concerns of `deref()` applies here.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(vec![1, 2, 3, 4]);
    /// let guard = &epoch::pin();
    ///
    /// let mut p = a.load(SeqCst, guard);
    /// unsafe {
    ///     assert!(!p.is_null());
    ///     let b = p.deref_mut();
    ///     assert_eq!(b, &vec![1, 2, 3, 4]);
    ///     b.push(5);
    ///     assert_eq!(b, &vec![1, 2, 3, 4, 5]);
    /// }
    ///
    /// let p = a.load(SeqCst, guard);
    /// unsafe {
    ///     assert_eq!(p.deref(), &vec![1, 2, 3, 4, 5]);
    /// }
    /// ```
    pub unsafe fn deref_mut(&mut self) -> &'g mut T {
        &mut *(self.as_raw() as *mut T)
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
    /// 1. A thread creates a new object: `a.store(Owned::new(10), Relaxed)`
    /// 2. Another thread reads it: `*a.load(Relaxed, guard).as_ref().unwrap()`
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
    /// let guard = &epoch::pin();
    /// let p = a.load(SeqCst, guard);
    /// unsafe {
    ///     assert_eq!(p.as_ref(), Some(&1234));
    /// }
    /// ```
    pub unsafe fn as_ref(&self) -> Option<&'g T> {
        self.as_raw().as_ref()
    }

    /// Takes ownership of the pointee.
    ///
    /// # Panics
    ///
    /// Panics if this pointer is null, but only in debug mode.
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
    ///     let guard = &epoch::unprotected();
    ///     let p = a.load(SeqCst, guard);
    ///     drop(p.into_owned());
    /// }
    /// ```
    pub unsafe fn into_owned(self) -> Owned<T> {
        debug_assert!(
            self.as_raw() != ptr::null(),
            "converting a null `Shared` into `Owned`"
        );
        Owned::from_usize(self.data)
    }

    /// Returns the tag stored within the pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Owned};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::<u64>::from(Owned::new(0u64).with_tag(2));
    /// let guard = &epoch::pin();
    /// let p = a.load(SeqCst, guard);
    /// assert_eq!(p.tag(), 2);
    /// ```
    pub fn tag(&self) -> usize {
        let (_, tag) = decompose_data::<T>(self.data);
        tag
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
    /// let guard = &epoch::pin();
    /// let p1 = a.load(SeqCst, guard);
    /// let p2 = p1.with_tag(2);
    ///
    /// assert_eq!(p1.tag(), 0);
    /// assert_eq!(p2.tag(), 2);
    /// assert_eq!(p1.as_raw(), p2.as_raw());
    /// ```
    pub fn with_tag(&self, tag: usize) -> Shared<'g, T> {
        unsafe { Self::from_usize(data_with_tag::<T>(self.data, tag)) }
    }
}

impl<'g, T> From<*const T> for Shared<'g, T> {
    /// Returns a new pointer pointing to `raw`.
    ///
    /// # Panics
    ///
    /// Panics if `raw` is not properly aligned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Shared;
    ///
    /// let p = unsafe { Shared::from(Box::into_raw(Box::new(1234)) as *const _) };
    /// assert!(!p.is_null());
    /// ```
    fn from(raw: *const T) -> Self {
        ensure_aligned(raw);
        unsafe { Self::from_usize(raw as usize) }
    }
}

impl<'g, T> PartialEq<Shared<'g, T>> for Shared<'g, T> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl<'g, T> Eq for Shared<'g, T> {}

impl<'g, T> PartialOrd<Shared<'g, T>> for Shared<'g, T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.data.partial_cmp(&other.data)
    }
}

impl<'g, T> Ord for Shared<'g, T> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.data.cmp(&other.data)
    }
}

impl<'g, T> fmt::Debug for Shared<'g, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (raw, tag) = decompose_data::<T>(self.data);

        f.debug_struct("Shared")
            .field("raw", &raw)
            .field("tag", &tag)
            .finish()
    }
}

impl<'g, T> fmt::Pointer for Shared<'g, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.as_raw(), f)
    }
}

impl<'g, T> Default for Shared<'g, T> {
    fn default() -> Self {
        Shared::null()
    }
}

#[cfg(test)]
mod tests {
    use super::Shared;

    #[test]
    fn valid_tag_i8() {
        Shared::<i8>::null().with_tag(0);
    }

    #[test]
    fn valid_tag_i64() {
        Shared::<i64>::null().with_tag(7);
    }
}
