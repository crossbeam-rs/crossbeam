use core::borrow::{Borrow, BorrowMut};
use core::cmp;
use core::fmt;
use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::slice;
use core::sync::atomic::Ordering;

use crate::alloc::alloc;
use crate::alloc::boxed::Box;
use crate::guard::Guard;
use crate::primitive::sync::atomic::AtomicPtr;
#[cfg(not(miri))]
use crate::primitive::sync::atomic::AtomicUsize;
use crossbeam_utils::atomic::AtomicConsume;

/// Given ordering for the success case in a compare-exchange operation, returns the strongest
/// appropriate ordering for the failure case.
#[cfg(miri)]
#[inline]
fn strongest_failure_ordering(order: Ordering) -> Ordering {
    use Ordering::*;
    match order {
        Relaxed | Release => Relaxed,
        Acquire | AcqRel => Acquire,
        _ => SeqCst,
    }
}

/// The error returned on failed compare-and-swap operation.
pub struct CompareExchangeError<'g, T: ?Sized + Pointable, P: Pointer<T>> {
    /// The value in the atomic pointer at the time of the failed operation.
    pub current: Shared<'g, T>,

    /// The new value, which the operation failed to store.
    pub new: P,
}

impl<T, P: Pointer<T> + fmt::Debug> fmt::Debug for CompareExchangeError<'_, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompareExchangeError")
            .field("current", &self.current)
            .field("new", &self.new)
            .finish()
    }
}

/// Returns a bitmask containing the unused least significant bits of an aligned pointer to `T`.
#[inline]
fn low_bits<T: ?Sized + Pointable>() -> usize {
    (1 << T::ALIGN.trailing_zeros()) - 1
}

/// Panics if the pointer is not properly unaligned.
#[inline]
fn ensure_aligned<T: ?Sized + Pointable>(raw: *mut ()) {
    assert_eq!(raw as usize & low_bits::<T>(), 0, "unaligned pointer");
}

/// Given a tagged pointer `data`, returns the same pointer, but tagged with `tag`.
///
/// `tag` is truncated to fit into the unused bits of the pointer to `T`.
#[inline]
fn compose_tag<T: ?Sized + Pointable>(ptr: *mut (), tag: usize) -> *mut () {
    int_to_ptr_with_provenance(
        (ptr as usize & !low_bits::<T>()) | (tag & low_bits::<T>()),
        ptr,
    )
}

/// Decomposes a tagged pointer `data` into the pointer and the tag.
#[inline]
fn decompose_tag<T: ?Sized + Pointable>(ptr: *mut ()) -> (*mut (), usize) {
    (
        int_to_ptr_with_provenance(ptr as usize & !low_bits::<T>(), ptr),
        ptr as usize & low_bits::<T>(),
    )
}

// HACK: https://github.com/rust-lang/miri/issues/1866#issuecomment-985802751
#[inline]
fn int_to_ptr_with_provenance<T>(addr: usize, prov: *mut T) -> *mut T {
    let ptr = prov.cast::<u8>();
    ptr.wrapping_add(addr.wrapping_sub(ptr as usize)).cast()
}

/// Types that are pointed to by a single word.
///
/// In concurrent programming, it is necessary to represent an object within a word because atomic
/// operations (e.g., reads, writes, read-modify-writes) support only single words.  This trait
/// qualifies such types that are pointed to by a single word.
///
/// The trait generalizes `Box<T>` for a sized type `T`.  In a box, an object of type `T` is
/// allocated in heap and it is owned by a single-word pointer.  This trait is also implemented for
/// `[MaybeUninit<T>]` by storing its size along with its elements and pointing to the pair of array
/// size and elements.
///
/// Pointers to `Pointable` types can be stored in [`Atomic`], [`Owned`], and [`Shared`].  In
/// particular, Crossbeam supports dynamically sized slices as follows.
///
/// ```
/// use std::mem::MaybeUninit;
/// use crossbeam_epoch::Owned;
///
/// let o = Owned::<[MaybeUninit<i32>]>::init(10); // allocating [i32; 10]
/// ```
pub trait Pointable {
    /// The alignment of pointer.
    const ALIGN: usize;

    /// The type for initializers.
    type Init;

    /// Initializes a with the given initializer.
    ///
    /// # Safety
    ///
    /// The result should be a multiple of `ALIGN`.
    unsafe fn init(init: Self::Init) -> *mut ();

    /// Dereferences the given pointer.
    ///
    /// # Safety
    ///
    /// - The given `ptr` should have been initialized with [`Pointable::init`].
    /// - `ptr` should not have yet been dropped by [`Pointable::drop`].
    /// - `ptr` should not be mutably dereferenced by [`Pointable::deref_mut`] concurrently.
    unsafe fn deref<'a>(ptr: *mut ()) -> &'a Self;

    /// Mutably dereferences the given pointer.
    ///
    /// # Safety
    ///
    /// - The given `ptr` should have been initialized with [`Pointable::init`].
    /// - `ptr` should not have yet been dropped by [`Pointable::drop`].
    /// - `ptr` should not be dereferenced by [`Pointable::deref`] or [`Pointable::deref_mut`]
    ///   concurrently.
    unsafe fn deref_mut<'a>(ptr: *mut ()) -> &'a mut Self;

    /// Drops the object pointed to by the given pointer.
    ///
    /// # Safety
    ///
    /// - The given `ptr` should have been initialized with [`Pointable::init`].
    /// - `ptr` should not have yet been dropped by [`Pointable::drop`].
    /// - `ptr` should not be dereferenced by [`Pointable::deref`] or [`Pointable::deref_mut`]
    ///   concurrently.
    unsafe fn drop(ptr: *mut ());
}

impl<T> Pointable for T {
    const ALIGN: usize = mem::align_of::<T>();

    type Init = T;

    unsafe fn init(init: Self::Init) -> *mut () {
        Box::into_raw(Box::new(init)).cast::<()>()
    }

    unsafe fn deref<'a>(ptr: *mut ()) -> &'a Self {
        &*(ptr as *const T)
    }

    unsafe fn deref_mut<'a>(ptr: *mut ()) -> &'a mut Self {
        &mut *ptr.cast::<T>()
    }

    unsafe fn drop(ptr: *mut ()) {
        drop(Box::from_raw(ptr.cast::<T>()));
    }
}

/// Array with size.
///
/// # Memory layout
///
/// An array consisting of size and elements:
///
/// ```text
///          elements
///          |
///          |
/// ------------------------------------
/// | size | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
/// ------------------------------------
/// ```
///
/// Its memory layout is different from that of `Box<[T]>` in that size is in the allocation (not
/// along with pointer as in `Box<[T]>`).
///
/// Elements are not present in the type, but they will be in the allocation.
/// ```
///
// TODO(@jeehoonkang): once we bump the minimum required Rust version to 1.44 or newer, use
// [`alloc::alloc::Layout::extend`] instead.
#[repr(C)]
struct Array<T> {
    /// The number of elements (not the number of bytes).
    len: usize,
    elements: [MaybeUninit<T>; 0],
}

impl<T> Pointable for [MaybeUninit<T>] {
    const ALIGN: usize = mem::align_of::<Array<T>>();

    type Init = usize;

    unsafe fn init(len: Self::Init) -> *mut () {
        let size = mem::size_of::<Array<T>>() + mem::size_of::<MaybeUninit<T>>() * len;
        let align = mem::align_of::<Array<T>>();
        let layout = alloc::Layout::from_size_align(size, align).unwrap();
        let ptr = alloc::alloc(layout).cast::<Array<T>>();
        if ptr.is_null() {
            alloc::handle_alloc_error(layout);
        }
        (*ptr).len = len;
        ptr.cast::<()>()
    }

    unsafe fn deref<'a>(ptr: *mut ()) -> &'a Self {
        let array = &*(ptr as *const Array<T>);
        slice::from_raw_parts(array.elements.as_ptr(), array.len)
    }

    unsafe fn deref_mut<'a>(ptr: *mut ()) -> &'a mut Self {
        let array = &mut *ptr.cast::<Array<T>>();
        slice::from_raw_parts_mut(array.elements.as_mut_ptr(), array.len)
    }

    unsafe fn drop(ptr: *mut ()) {
        let array = &*ptr.cast::<Array<T>>();
        let size = mem::size_of::<Array<T>>() + mem::size_of::<MaybeUninit<T>>() * array.len;
        let align = mem::align_of::<Array<T>>();
        let layout = alloc::Layout::from_size_align(size, align).unwrap();
        alloc::dealloc(ptr.cast::<u8>(), layout);
    }
}

/// An atomic pointer that can be safely shared between threads.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address. For example, the tag for a pointer to a sized type `T`
/// should be less than `(1 << mem::align_of::<T>().trailing_zeros())`.
///
/// Any method that loads the pointer must be passed a reference to a [`Guard`].
///
/// Crossbeam supports dynamically sized types.  See [`Pointable`] for details.
pub struct Atomic<T: ?Sized + Pointable> {
    data: AtomicPtr<()>,
    _marker: PhantomData<*mut T>,
}

unsafe impl<T: ?Sized + Pointable + Send + Sync> Send for Atomic<T> {}
unsafe impl<T: ?Sized + Pointable + Send + Sync> Sync for Atomic<T> {}

impl<T> Atomic<T> {
    /// Allocates `value` on the heap and returns a new atomic pointer pointing to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Atomic;
    ///
    /// let a = Atomic::new(1234);
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn new(init: T) -> Atomic<T> {
        Self::init(init)
    }
}

impl<T: ?Sized + Pointable> Atomic<T> {
    /// Allocates `value` on the heap and returns a new atomic pointer pointing to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Atomic;
    ///
    /// let a = Atomic::<i32>::init(1234);
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn init(init: T::Init) -> Atomic<T> {
        Self::from(Owned::init(init))
    }

    /// Returns a new atomic pointer pointing to the tagged pointer `data`.
    fn from_ptr(data: *mut ()) -> Self {
        Self {
            data: AtomicPtr::new(data),
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
    #[cfg(all(not(crossbeam_no_const_fn_trait_bound), not(crossbeam_loom)))]
    pub const fn null() -> Atomic<T> {
        Self {
            data: AtomicPtr::new(ptr::null_mut()),
            _marker: PhantomData,
        }
    }

    /// Returns a new null atomic pointer.
    #[cfg(not(all(not(crossbeam_no_const_fn_trait_bound), not(crossbeam_loom))))]
    pub fn null() -> Atomic<T> {
        Self {
            data: AtomicPtr::new(ptr::null_mut()),
            _marker: PhantomData,
        }
    }

    /// Loads a `Shared` from the atomic pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn load<'g>(&self, order: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_ptr(self.data.load(order)) }
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn load_consume<'g>(&self, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_ptr(self.data.load_consume()) }
    }

    /// Stores a `Shared` or `Owned` pointer into the atomic pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{Atomic, Owned, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// # unsafe { drop(a.load(SeqCst, &crossbeam_epoch::pin()).into_owned()); } // avoid leak
    /// a.store(Shared::null(), SeqCst);
    /// a.store(Owned::new(1234), SeqCst);
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn store<P: Pointer<T>>(&self, new: P, order: Ordering) {
        self.data.store(new.into_ptr(), order);
    }

    /// Stores a `Shared` or `Owned` pointer into the atomic pointer, returning the previous
    /// `Shared`.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic, Shared};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// let guard = &epoch::pin();
    /// let p = a.swap(Shared::null(), SeqCst, guard);
    /// # unsafe { drop(p.into_owned()); } // avoid leak
    /// ```
    pub fn swap<'g, P: Pointer<T>>(&self, new: P, order: Ordering, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_ptr(self.data.swap(new.into_ptr(), order)) }
    }

    /// Stores the pointer `new` (either `Shared` or `Owned`) into the atomic pointer if the current
    /// value is the same as `current`. The tag is also taken into account, so two pointers to the
    /// same object, but with different tags, will not be considered equal.
    ///
    /// The return value is a result indicating whether the new pointer was written. On success the
    /// pointer that was written is returned. On failure the actual current value and `new` are
    /// returned.
    ///
    /// This method takes two `Ordering` arguments to describe the memory
    /// ordering of this operation. `success` describes the required ordering for the
    /// read-modify-write operation that takes place if the comparison with `current` succeeds.
    /// `failure` describes the required ordering for the load operation that takes place when
    /// the comparison fails. Using `Acquire` as success ordering makes the store part
    /// of this operation `Relaxed`, and using `Release` makes the successful load
    /// `Relaxed`. The failure ordering can only be `SeqCst`, `Acquire` or `Relaxed`
    /// and must be equivalent to or weaker than the success ordering.
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
    /// let curr = a.load(SeqCst, guard);
    /// let res1 = a.compare_exchange(curr, Shared::null(), SeqCst, SeqCst, guard);
    /// let res2 = a.compare_exchange(curr, Owned::new(5678), SeqCst, SeqCst, guard);
    /// # unsafe { drop(curr.into_owned()); } // avoid leak
    /// ```
    pub fn compare_exchange<'g, P>(
        &self,
        current: Shared<'_, T>,
        new: P,
        success: Ordering,
        failure: Ordering,
        _: &'g Guard,
    ) -> Result<Shared<'g, T>, CompareExchangeError<'g, T, P>>
    where
        P: Pointer<T>,
    {
        let new = new.into_ptr();
        self.data
            .compare_exchange(current.into_ptr(), new, success, failure)
            .map(|_| unsafe { Shared::from_ptr(new) })
            .map_err(|current| unsafe {
                CompareExchangeError {
                    current: Shared::from_ptr(current),
                    new: P::from_ptr(new),
                }
            })
    }

    /// Stores the pointer `new` (either `Shared` or `Owned`) into the atomic pointer if the current
    /// value is the same as `current`. The tag is also taken into account, so two pointers to the
    /// same object, but with different tags, will not be considered equal.
    ///
    /// Unlike [`compare_exchange`], this method is allowed to spuriously fail even when comparison
    /// succeeds, which can result in more efficient code on some platforms.  The return value is a
    /// result indicating whether the new pointer was written. On success the pointer that was
    /// written is returned. On failure the actual current value and `new` are returned.
    ///
    /// This method takes two `Ordering` arguments to describe the memory
    /// ordering of this operation. `success` describes the required ordering for the
    /// read-modify-write operation that takes place if the comparison with `current` succeeds.
    /// `failure` describes the required ordering for the load operation that takes place when
    /// the comparison fails. Using `Acquire` as success ordering makes the store part
    /// of this operation `Relaxed`, and using `Release` makes the successful load
    /// `Relaxed`. The failure ordering can only be `SeqCst`, `Acquire` or `Relaxed`
    /// and must be equivalent to or weaker than the success ordering.
    ///
    /// [`compare_exchange`]: Atomic::compare_exchange
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
    /// # unsafe { drop(a.load(SeqCst, guard).into_owned()); } // avoid leak
    /// loop {
    ///     match a.compare_exchange_weak(ptr, new, SeqCst, SeqCst, guard) {
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
    ///     match a.compare_exchange_weak(curr, Shared::null(), SeqCst, SeqCst, guard) {
    ///         Ok(_) => break,
    ///         Err(err) => curr = err.current,
    ///     }
    /// }
    /// # unsafe { drop(curr.into_owned()); } // avoid leak
    /// ```
    pub fn compare_exchange_weak<'g, P>(
        &self,
        current: Shared<'_, T>,
        new: P,
        success: Ordering,
        failure: Ordering,
        _: &'g Guard,
    ) -> Result<Shared<'g, T>, CompareExchangeError<'g, T, P>>
    where
        P: Pointer<T>,
    {
        let new = new.into_ptr();
        self.data
            .compare_exchange_weak(current.into_ptr(), new, success, failure)
            .map(|_| unsafe { Shared::from_ptr(new) })
            .map_err(|current| unsafe {
                CompareExchangeError {
                    current: Shared::from_ptr(current),
                    new: P::from_ptr(new),
                }
            })
    }

    /// Fetches the pointer, and then applies a function to it that returns a new value.
    /// Returns a `Result` of `Ok(previous_value)` if the function returned `Some`, else `Err(_)`.
    ///
    /// Note that the given function may be called multiple times if the value has been changed by
    /// other threads in the meantime, as long as the function returns `Some(_)`, but the function
    /// will have been applied only once to the stored value.
    ///
    /// `fetch_update` takes two [`Ordering`] arguments to describe the memory
    /// ordering of this operation. The first describes the required ordering for
    /// when the operation finally succeeds while the second describes the
    /// required ordering for loads. These correspond to the success and failure
    /// orderings of [`Atomic::compare_exchange`] respectively.
    ///
    /// Using [`Acquire`] as success ordering makes the store part of this
    /// operation [`Relaxed`], and using [`Release`] makes the final successful
    /// load [`Relaxed`]. The (failed) load ordering can only be [`SeqCst`],
    /// [`Acquire`] or [`Relaxed`] and must be equivalent to or weaker than the
    /// success ordering.
    ///
    /// [`Relaxed`]: Ordering::Relaxed
    /// [`Acquire`]: Ordering::Acquire
    /// [`Release`]: Ordering::Release
    /// [`SeqCst`]: Ordering::SeqCst
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{self as epoch, Atomic};
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// let a = Atomic::new(1234);
    /// let guard = &epoch::pin();
    ///
    /// let res1 = a.fetch_update(SeqCst, SeqCst, guard, |x| Some(x.with_tag(1)));
    /// assert!(res1.is_ok());
    ///
    /// let res2 = a.fetch_update(SeqCst, SeqCst, guard, |x| None);
    /// assert!(res2.is_err());
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn fetch_update<'g, F>(
        &self,
        set_order: Ordering,
        fail_order: Ordering,
        guard: &'g Guard,
        mut func: F,
    ) -> Result<Shared<'g, T>, Shared<'g, T>>
    where
        F: FnMut(Shared<'g, T>) -> Option<Shared<'g, T>>,
    {
        let mut prev = self.load(fail_order, guard);
        while let Some(next) = func(prev) {
            match self.compare_exchange_weak(prev, next, set_order, fail_order, guard) {
                Ok(shared) => return Ok(shared),
                Err(next_prev) => prev = next_prev.current,
            }
        }
        Err(prev)
    }

    /// Bitwise "and" with the current tag.
    ///
    /// Performs a bitwise "and" operation on the current tag and the argument `val`, and sets the
    /// new tag to the result. Returns the previous pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
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
    pub fn fetch_and<'g>(&self, val: usize, order: Ordering, _: &'g Guard) -> Shared<'g, T> {
        // Ideally, we would always use AtomicPtr::fetch_* since it is strict-provenance
        // compatible, but it is unstable. So, for now emulate it only on cfg(miri).
        // Code using AtomicUsize::fetch_* via casts is still permissive-provenance
        // compatible and is sound.
        // TODO: Once `#![feature(strict_provenance_atomic_ptr)]` is stabilized,
        // use AtomicPtr::fetch_* in all cases from the version in which it is stabilized.
        #[cfg(miri)]
        unsafe {
            let val = val | !low_bits::<T>();
            let fetch_order = strongest_failure_ordering(order);
            Shared::from_ptr(
                self.data
                    .fetch_update(order, fetch_order, |x| {
                        Some(int_to_ptr_with_provenance(x as usize & val, x))
                    })
                    .unwrap(),
            )
        }
        #[cfg(not(miri))]
        unsafe {
            Shared::from_ptr(
                (*(&self.data as *const AtomicPtr<_> as *const AtomicUsize))
                    .fetch_and(val | !low_bits::<T>(), order) as *mut (),
            )
        }
    }

    /// Bitwise "or" with the current tag.
    ///
    /// Performs a bitwise "or" operation on the current tag and the argument `val`, and sets the
    /// new tag to the result. Returns the previous pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
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
    pub fn fetch_or<'g>(&self, val: usize, order: Ordering, _: &'g Guard) -> Shared<'g, T> {
        // Ideally, we would always use AtomicPtr::fetch_* since it is strict-provenance
        // compatible, but it is unstable. So, for now emulate it only on cfg(miri).
        // Code using AtomicUsize::fetch_* via casts is still permissive-provenance
        // compatible and is sound.
        // TODO: Once `#![feature(strict_provenance_atomic_ptr)]` is stabilized,
        // use AtomicPtr::fetch_* in all cases from the version in which it is stabilized.
        #[cfg(miri)]
        unsafe {
            let val = val & low_bits::<T>();
            let fetch_order = strongest_failure_ordering(order);
            Shared::from_ptr(
                self.data
                    .fetch_update(order, fetch_order, |x| {
                        Some(int_to_ptr_with_provenance(x as usize | val, x))
                    })
                    .unwrap(),
            )
        }
        #[cfg(not(miri))]
        unsafe {
            Shared::from_ptr(
                (*(&self.data as *const AtomicPtr<_> as *const AtomicUsize))
                    .fetch_or(val & low_bits::<T>(), order) as *mut (),
            )
        }
    }

    /// Bitwise "xor" with the current tag.
    ///
    /// Performs a bitwise "xor" operation on the current tag and the argument `val`, and sets the
    /// new tag to the result. Returns the previous pointer.
    ///
    /// This method takes an [`Ordering`] argument which describes the memory ordering of this
    /// operation.
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
    pub fn fetch_xor<'g>(&self, val: usize, order: Ordering, _: &'g Guard) -> Shared<'g, T> {
        // Ideally, we would always use AtomicPtr::fetch_* since it is strict-provenance
        // compatible, but it is unstable. So, for now emulate it only on cfg(miri).
        // Code using AtomicUsize::fetch_* via casts is still permissive-provenance
        // compatible and is sound.
        // TODO: Once `#![feature(strict_provenance_atomic_ptr)]` is stabilized,
        // use AtomicPtr::fetch_* in all cases from the version in which it is stabilized.
        #[cfg(miri)]
        unsafe {
            let val = val & low_bits::<T>();
            let fetch_order = strongest_failure_ordering(order);
            Shared::from_ptr(
                self.data
                    .fetch_update(order, fetch_order, |x| {
                        Some(int_to_ptr_with_provenance(x as usize ^ val, x))
                    })
                    .unwrap(),
            )
        }
        #[cfg(not(miri))]
        unsafe {
            Shared::from_ptr(
                (*(&self.data as *const AtomicPtr<_> as *const AtomicUsize))
                    .fetch_xor(val & low_bits::<T>(), order) as *mut (),
            )
        }
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
        #[cfg(crossbeam_loom)]
        {
            // FIXME: loom does not yet support into_inner, so we use unsync_load for now,
            // which should have the same synchronization properties:
            // https://github.com/tokio-rs/loom/issues/117
            Owned::from_ptr(self.data.unsync_load())
        }
        #[cfg(not(crossbeam_loom))]
        {
            Owned::from_ptr(self.data.into_inner())
        }
    }

    /// Takes ownership of the pointee if it is non-null.
    ///
    /// This consumes the atomic and converts it into [`Owned`]. As [`Atomic`] doesn't have a
    /// destructor and doesn't drop the pointee while [`Owned`] does, this is suitable for
    /// destructors of data structures.
    ///
    /// # Safety
    ///
    /// This method may be called only if the pointer is valid and nobody else is holding a
    /// reference to the same object, or the pointer is null.
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
    ///         // any Shared or & to it ourselves, but it may be null, so we have to be careful.
    ///         let old = mem::replace(&mut self.ptr, Atomic::null());
    ///         unsafe {
    ///             if let Some(x) = old.try_into_owned() {
    ///                 drop(x)
    ///             }
    ///         }
    ///     }
    /// }
    /// ```
    pub unsafe fn try_into_owned(self) -> Option<Owned<T>> {
        // FIXME: See self.into_owned()
        #[cfg(crossbeam_loom)]
        let data = self.data.unsync_load();
        #[cfg(not(crossbeam_loom))]
        let data = self.data.into_inner();
        if decompose_tag::<T>(data).0.is_null() {
            None
        } else {
            Some(Owned::from_ptr(data))
        }
    }
}

impl<T: ?Sized + Pointable> fmt::Debug for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = self.data.load(Ordering::SeqCst);
        let (raw, tag) = decompose_tag::<T>(data);

        f.debug_struct("Atomic")
            .field("raw", &raw)
            .field("tag", &tag)
            .finish()
    }
}

impl<T: ?Sized + Pointable> fmt::Pointer for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let data = self.data.load(Ordering::SeqCst);
        let (raw, _) = decompose_tag::<T>(data);
        fmt::Pointer::fmt(&(unsafe { T::deref(raw) as *const _ }), f)
    }
}

impl<T: ?Sized + Pointable> Clone for Atomic<T> {
    /// Returns a copy of the atomic value.
    ///
    /// Note that a `Relaxed` load is used here. If you need synchronization, use it with other
    /// atomics or fences.
    fn clone(&self) -> Self {
        let data = self.data.load(Ordering::Relaxed);
        Atomic::from_ptr(data)
    }
}

impl<T: ?Sized + Pointable> Default for Atomic<T> {
    fn default() -> Self {
        Atomic::null()
    }
}

impl<T: ?Sized + Pointable> From<Owned<T>> for Atomic<T> {
    /// Returns a new atomic pointer pointing to `owned`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::{Atomic, Owned};
    ///
    /// let a = Atomic::<i32>::from(Owned::new(1234));
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    fn from(owned: Owned<T>) -> Self {
        let data = owned.data;
        mem::forget(owned);
        Self::from_ptr(data)
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

impl<'g, T: ?Sized + Pointable> From<Shared<'g, T>> for Atomic<T> {
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
        Self::from_ptr(ptr.data)
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
        Self::from_ptr(raw as *mut ())
    }
}

/// A trait for either `Owned` or `Shared` pointers.
///
/// This trait is sealed and cannot be implemented for types outside of `crossbeam-epoch`.
pub trait Pointer<T: ?Sized + Pointable>: crate::sealed::Sealed {
    /// Returns the machine representation of the pointer.
    fn into_ptr(self) -> *mut ();

    /// Returns a new pointer pointing to the tagged pointer `data`.
    ///
    /// # Safety
    ///
    /// The given `data` should have been created by `Pointer::into_ptr()`, and one `data` should
    /// not be converted back by `Pointer::from_ptr()` multiple times.
    unsafe fn from_ptr(data: *mut ()) -> Self;
}

/// An owned heap-allocated object.
///
/// This type is very similar to `Box<T>`.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.
pub struct Owned<T: ?Sized + Pointable> {
    data: *mut (),
    _marker: PhantomData<Box<T>>,
}

impl<T: ?Sized + Pointable> crate::sealed::Sealed for Owned<T> {}
impl<T: ?Sized + Pointable> Pointer<T> for Owned<T> {
    #[inline]
    fn into_ptr(self) -> *mut () {
        let data = self.data;
        mem::forget(self);
        data
    }

    /// Returns a new pointer pointing to the tagged pointer `data`.
    ///
    /// # Panics
    ///
    /// Panics if the pointer is null, but only in debug mode.
    #[inline]
    unsafe fn from_ptr(data: *mut ()) -> Self {
        debug_assert!(!data.is_null(), "converting null into `Owned`");
        Self {
            data,
            _marker: PhantomData,
        }
    }
}

impl<T> Owned<T> {
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
    /// # Safety
    ///
    /// The given `raw` should have been derived from `Owned`, and one `raw` should not be converted
    /// back by `Owned::from_raw()` multiple times.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = unsafe { Owned::from_raw(Box::into_raw(Box::new(1234))) };
    /// ```
    pub unsafe fn from_raw(raw: *mut T) -> Owned<T> {
        let raw = raw.cast::<()>();
        ensure_aligned::<T>(raw);
        Self::from_ptr(raw)
    }

    /// Converts the owned pointer into a `Box`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = Owned::new(1234);
    /// let b: Box<i32> = o.into_box();
    /// assert_eq!(*b, 1234);
    /// ```
    pub fn into_box(self) -> Box<T> {
        let (raw, _) = decompose_tag::<T>(self.data);
        mem::forget(self);
        unsafe { Box::from_raw(raw.cast::<T>()) }
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
    pub fn new(init: T) -> Owned<T> {
        Self::init(init)
    }
}

impl<T: ?Sized + Pointable> Owned<T> {
    /// Allocates `value` on the heap and returns a new owned pointer pointing to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_epoch::Owned;
    ///
    /// let o = Owned::<i32>::init(1234);
    /// ```
    pub fn init(init: T::Init) -> Owned<T> {
        unsafe { Self::from_ptr(T::init(init)) }
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
    /// # unsafe { drop(p.into_owned()); } // avoid leak
    /// ```
    #[allow(clippy::needless_lifetimes)]
    pub fn into_shared<'g>(self, _: &'g Guard) -> Shared<'g, T> {
        unsafe { Shared::from_ptr(self.into_ptr()) }
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
        let (_, tag) = decompose_tag::<T>(self.data);
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
        let data = self.into_ptr();
        unsafe { Self::from_ptr(compose_tag::<T>(data, tag)) }
    }
}

impl<T: ?Sized + Pointable> Drop for Owned<T> {
    fn drop(&mut self) {
        let (raw, _) = decompose_tag::<T>(self.data);
        unsafe {
            T::drop(raw);
        }
    }
}

impl<T: ?Sized + Pointable> fmt::Debug for Owned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (raw, tag) = decompose_tag::<T>(self.data);

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

impl<T: ?Sized + Pointable> Deref for Owned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        let (raw, _) = decompose_tag::<T>(self.data);
        unsafe { T::deref(raw) }
    }
}

impl<T: ?Sized + Pointable> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        let (raw, _) = decompose_tag::<T>(self.data);
        unsafe { T::deref_mut(raw) }
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

impl<T: ?Sized + Pointable> Borrow<T> for Owned<T> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<T: ?Sized + Pointable> BorrowMut<T> for Owned<T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}

impl<T: ?Sized + Pointable> AsRef<T> for Owned<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T: ?Sized + Pointable> AsMut<T> for Owned<T> {
    fn as_mut(&mut self) -> &mut T {
        self.deref_mut()
    }
}

/// A pointer to an object protected by the epoch GC.
///
/// The pointer is valid for use only during the lifetime `'g`.
///
/// The pointer must be properly aligned. Since it is aligned, a tag can be stored into the unused
/// least significant bits of the address.
pub struct Shared<'g, T: 'g + ?Sized + Pointable> {
    data: *mut (),
    _marker: PhantomData<(&'g (), *const T)>,
}

impl<T: ?Sized + Pointable> Clone for Shared<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized + Pointable> Copy for Shared<'_, T> {}

impl<T: ?Sized + Pointable> crate::sealed::Sealed for Shared<'_, T> {}
impl<T: ?Sized + Pointable> Pointer<T> for Shared<'_, T> {
    #[inline]
    fn into_ptr(self) -> *mut () {
        self.data
    }

    #[inline]
    unsafe fn from_ptr(data: *mut ()) -> Self {
        Shared {
            data,
            _marker: PhantomData,
        }
    }
}

impl<'g, T> Shared<'g, T> {
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn as_raw(&self) -> *const T {
        let (raw, _) = decompose_tag::<T>(self.data);
        raw as *const _
    }
}

impl<'g, T: ?Sized + Pointable> Shared<'g, T> {
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
            data: ptr::null_mut(),
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn is_null(&self) -> bool {
        let (raw, _) = decompose_tag::<T>(self.data);
        raw.is_null()
    }

    /// Dereferences the pointer.
    ///
    /// Returns a reference to the pointee that is valid during the lifetime `'g`.
    ///
    /// # Safety
    ///
    /// Dereferencing a pointer is unsafe because it could be pointing to invalid memory.
    ///
    /// Another concern is the possibility of data races due to lack of proper synchronization.
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub unsafe fn deref(&self) -> &'g T {
        let (raw, _) = decompose_tag::<T>(self.data);
        T::deref(raw)
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub unsafe fn deref_mut(&mut self) -> &'g mut T {
        let (raw, _) = decompose_tag::<T>(self.data);
        T::deref_mut(raw)
    }

    /// Converts the pointer to a reference.
    ///
    /// Returns `None` if the pointer is null, or else a reference to the object wrapped in `Some`.
    ///
    /// # Safety
    ///
    /// Dereferencing a pointer is unsafe because it could be pointing to invalid memory.
    ///
    /// Another concern is the possibility of data races due to lack of proper synchronization.
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub unsafe fn as_ref(&self) -> Option<&'g T> {
        let (raw, _) = decompose_tag::<T>(self.data);
        if raw.is_null() {
            None
        } else {
            Some(T::deref(raw))
        }
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
        debug_assert!(!self.is_null(), "converting a null `Shared` into `Owned`");
        Owned::from_ptr(self.data)
    }

    /// Takes ownership of the pointee if it is not null.
    ///
    /// # Safety
    ///
    /// This method may be called only if the pointer is valid and nobody else is holding a
    /// reference to the same object, or if the pointer is null.
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
    ///     if let Some(x) = p.try_into_owned() {
    ///         drop(x);
    ///     }
    /// }
    /// ```
    pub unsafe fn try_into_owned(self) -> Option<Owned<T>> {
        if self.is_null() {
            None
        } else {
            Some(Owned::from_ptr(self.data))
        }
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn tag(&self) -> usize {
        let (_, tag) = decompose_tag::<T>(self.data);
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
    /// # unsafe { drop(a.into_owned()); } // avoid leak
    /// ```
    pub fn with_tag(&self, tag: usize) -> Shared<'g, T> {
        unsafe { Self::from_ptr(compose_tag::<T>(self.data, tag)) }
    }
}

impl<T> From<*const T> for Shared<'_, T> {
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
    /// let p = Shared::from(Box::into_raw(Box::new(1234)) as *const _);
    /// assert!(!p.is_null());
    /// # unsafe { drop(p.into_owned()); } // avoid leak
    /// ```
    fn from(raw: *const T) -> Self {
        let raw = raw as *mut ();
        ensure_aligned::<T>(raw);
        unsafe { Self::from_ptr(raw) }
    }
}

impl<'g, T: ?Sized + Pointable> PartialEq<Shared<'g, T>> for Shared<'g, T> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl<T: ?Sized + Pointable> Eq for Shared<'_, T> {}

impl<'g, T: ?Sized + Pointable> PartialOrd<Shared<'g, T>> for Shared<'g, T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.data.cmp(&other.data))
    }
}

impl<T: ?Sized + Pointable> Ord for Shared<'_, T> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.data.cmp(&other.data)
    }
}

impl<T: ?Sized + Pointable> fmt::Debug for Shared<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (raw, tag) = decompose_tag::<T>(self.data);

        f.debug_struct("Shared")
            .field("raw", &raw)
            .field("tag", &tag)
            .finish()
    }
}

impl<T: ?Sized + Pointable> fmt::Pointer for Shared<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&(unsafe { self.deref() as *const _ }), f)
    }
}

impl<T: ?Sized + Pointable> Default for Shared<'_, T> {
    fn default() -> Self {
        Shared::null()
    }
}

#[cfg(all(test, not(crossbeam_loom)))]
mod tests {
    use super::{Owned, Shared};
    use std::mem::MaybeUninit;

    #[test]
    fn valid_tag_i8() {
        Shared::<i8>::null().with_tag(0);
    }

    #[test]
    fn valid_tag_i64() {
        Shared::<i64>::null().with_tag(7);
    }

    #[rustversion::since(1.61)]
    #[test]
    fn const_atomic_null() {
        use super::Atomic;
        static _U: Atomic<u8> = Atomic::<u8>::null();
    }

    #[test]
    fn array_init() {
        let owned = Owned::<[MaybeUninit<usize>]>::init(10);
        let arr: &[MaybeUninit<usize>] = &owned;
        assert_eq!(arr.len(), 10);
    }
}
