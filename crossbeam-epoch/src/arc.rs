//! Coalesced atomic reference counting with epoch-based deferment
//!
//! A thread-local _ledger_ of outstanding increments and decrements is
//! maintained.  At the end of a _pinned_ scope, all pending increments are
//! performed, and all pending decrements are handed off to the crossbeam
//! garbage collector for execution after all other currently pinned threads
//! have unpinned.  Matching increments and decrements in the same scope, which
//! correspond to objects that lived only briefly, can be cancelled out and
//! never need to be applied at all.
//!
//! When a thread is not pinned, `Arc`s may still be cloned and dropped, with
//! increments applied immediately and decrements deferred until after the next
//! time the thread is pinned.
//!
//! Performance hinges on the efficiency of the thread-local ledger; it must be
//! significantly cheaper to perform this bookkeeping than to perform eager and
//! sometimes redundant atomic operations, at least in the case of high
//! contention.
//!
//! To use the ledger even less, atomic loads produce borrows with lifetimes
//! limited to either the epoch::guard or the non-atomic donor; these don't have
//! to touch the ledger unless they are written into an owning object somewhere;
//! notably in the load-compare-exchange pattern the expected values may never
//! need to touch the ledger

use crate::default::with_handle;
use crate::{internal::Local, CompareAndSetOrdering, Guard};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell, UnsafeCell};
use core::marker::PhantomData;
use core::mem::{forget, ManuallyDrop};
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use crossbeam_utils::atomic::AtomicConsume;
use std::collections::hash_map::{
    Entry::{Occupied, Vacant},
    HashMap,
};

/// Compile-time (?) mask for alignment bits of a pointer
const fn tag_mask<T>() -> usize {
    // To convert a power-of-two alignment to a mask, subtract one.
    std::mem::align_of::<T>() - 1
}

/// Compile-time (?) mask for non-alignment bits of a pointer
const fn ptr_mask<T>() -> usize {
    !tag_mask::<T>()
}

/// A `T` and its atomic reference counting metadata, intended for heap
/// allocation and management with a reference-counting smart pointer
#[derive(Debug)]
struct Inner<T> {
    strong: AtomicUsize,
    weak: AtomicUsize,
    data: UnsafeCell<ManuallyDrop<T>>,
}

/// Points to an `Inner<_>` and provides reference counting services for
/// user-facing types like `Arc<_>`
#[derive(Debug)]
struct Pointer<T> {
    ptr: NonNull<Inner<T>>,
    _marker: PhantomData<Inner<T>>,
}

impl<T> Pointer<T> {
    fn add_strong(&self, n: usize) {
        debug_assert_ne!(n, 0);
        let old = self.strong().fetch_add(n, Ordering::Relaxed);
        debug_assert_ne!(old, 0);
    }

    fn add_weak(&self, n: usize) {
        debug_assert_ne!(n, 0);
        let old = self.weak().fetch_add(n, Ordering::Relaxed);
        debug_assert_ne!(old, 0);
    }

    fn as_usize(&self) -> usize {
        let data = self.ptr.as_ptr() as usize;
        // Assert non-null
        debug_assert_ne!(data & ptr_mask::<Inner<T>>(), 0);
        // Assert aligned
        debug_assert_eq!(data & tag_mask::<Inner<T>>(), 0);
        data
    }

    unsafe fn dealloc(&self) {
        alloc::boxed::Box::from_raw(self.ptr.as_ptr());
    }

    fn defer_decr_strong(&self) {
        with_ledger(|x, _| x.defer_decr_strong(*self))
    }

    fn defer_decr_weak(&self) {
        with_ledger(|x, _| x.defer_decr_weak(*self))
    }

    fn defer_incr_strong(&self) {
        with_ledger(|x, is_pinned| x.defer_incr_strong(*self, is_pinned))
    }

    fn defer_incr_weak(&self) {
        with_ledger(|x, is_pinned| x.defer_incr_weak(*self, is_pinned))
    }

    fn deref(&self) -> &T {
        unsafe { &**self.ptr.as_ref().data.get() }
    }

    /// Safety: We must be the only pointer to `Inner`, either because we are a
    /// `Box`, or a newly constructed `Arc` that has not yet been shared.
    /// Deferred increments mean that we cannot use the strong count to
    /// establish uniqueness.
    unsafe fn deref_mut(&mut self) -> &mut T {
        &mut **self.ptr.as_ref().data.get()
    }

    fn downgrade(&self) -> Weak<T> {
        self.defer_incr_weak();
        Weak { ptr: *self }
    }

    /// Safety: We must have been last strong owner of the `Inner`, either
    /// because we just decreased the strong count to zero, or because we are a
    /// `Box`.
    unsafe fn drop_data(&self) {
        ManuallyDrop::drop(&mut *self.ptr.as_ref().data.get());
    }

    /// Safety: The argument must have originated from `as_usize` or
    /// `into_usize`.  In debug mode, non-null-ness and alignment are checked.
    unsafe fn from_usize(data: usize) -> Self {
        // Assert non-null
        debug_assert_ne!(data & ptr_mask::<Inner<T>>(), 0);
        // Assert aligned
        debug_assert_eq!(data & tag_mask::<Inner<T>>(), 0);
        Self {
            ptr: NonNull::new_unchecked(data as *mut Inner<T>),
            _marker: PhantomData,
        }
    }

    fn new(data: T) -> Self {
        Self {
            ptr: {
                let p = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(Inner {
                    strong: AtomicUsize::new(1),
                    weak: AtomicUsize::new(1),
                    data: UnsafeCell::new(ManuallyDrop::new(data)),
                }));
                // Safety: the pointer returned by Box::into_raw is non-null,
                // aligned and dereferenceable
                unsafe { NonNull::new_unchecked(p) }
            },
            _marker: PhantomData,
        }
    }

    /// Convenience accessor
    fn strong(&self) -> &AtomicUsize {
        // Safety: the pointer is valid
        unsafe { &self.ptr.as_ref().strong }
    }

    /// Gets the strong count.  Deferred increments and decrements make this
    /// value hard to interpret.  A strong_count observed to be zero will remain
    /// zero for the rest of the lifetime of the `Inner`; such an observation
    /// should only be possible through a `Weak` (or `BorrowedWeak`) pointer.
    ///
    /// # Safety
    ///
    /// Safe to call but hard to use safely
    fn strong_count(&self) -> usize {
        self.strong().load(Ordering::Relaxed)
    }

    /// Subtract from the strong count, possibly destroying dropping the payload
    /// and deallocating the `Inner`
    ///
    /// # Safety
    ///
    /// Must be called with n less than or equal to the current strong count;
    /// must be called only after pinned threads that have observed payload have
    /// unpinned; caller must not dereference their pointer afterwards
    unsafe fn sub_strong(&self, n: usize) {
        debug_assert_ne!(n, 0);
        let old = self.strong().fetch_sub(n, Ordering::Release);
        debug_assert!(old >= n);
        if old == n {
            let old = self.strong().load(Ordering::Acquire);
            debug_assert_eq!(old, 0);
            self.drop_data();
            self.sub_weak(1);
        }
    }

    /// Subtract from the weak count, possibly deallocating the `Inner`
    ///
    /// # Safety
    ///
    unsafe fn sub_weak(&self, n: usize) {
        debug_assert_ne!(n, 0);
        let old = self.weak().fetch_sub(n, Ordering::Release);
        debug_assert!(old >= n);
        if old == n {
            let old = self.weak().load(Ordering::Acquire);
            debug_assert_eq!(old, 0);
            self.dealloc();
        }
    }

    /// Conditionally upgrade a `Weak` (or any other class of pointer) into an
    /// `Arc` if the strong count is nonzero.  This reference count manipulation
    /// happens immediately, unlike the rest of this library.  This operation
    /// exposes the deferred decrements; it may succeed even after all strong
    /// pointers are unreachable if garbage collection has not yet occurred.
    ///
    /// # Safety
    ///
    /// Always safe to call as pointer always points to valid metadata
    fn upgrade(&self) -> Option<Arc<T>> {
        let mut expected = self.strong().load(Ordering::Relaxed);
        while expected != 0 {
            match self.strong().compare_exchange_weak(
                expected,
                expected + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Arc { ptr: *self }),
                Err(unexpected) => expected = unexpected,
            }
        }
        None
    }

    /// Convenience method
    fn weak(&self) -> &AtomicUsize {
        // Safety: Always safe to access metadata
        unsafe { &self.ptr.as_ref().weak }
    }

    /// Get the weak reference count.  Deferred increments and decrements mean
    /// that this value is hard to interpret.  A value of zero should never be
    /// observed.
    ///
    /// # Safety
    ///
    /// Safe to call, no way to use safely.  Only for debugging
    fn weak_count(&self) -> usize {
        let old = self.weak().load(Ordering::Relaxed);
        debug_assert_ne!(old, 0);
        old
    }
}

impl<T> Copy for Pointer<T> {}

impl<T> Clone for Pointer<T> {
    fn clone(&self) -> Self {
        *self
    }
}

/// A mutable, uniquely owned heap allocation like `alloc::boxed::Box` but with
/// the necessary metadata to be converted into an immutable, shared heap
/// allocation like `alloc::sync::Arc`.  (Name likely to be controversial?)
///
/// The difficulty of determining when an `Arc` is safely mutable when reference
/// counting is deferred make this convenience type more important than for
/// vanilla `Arc` where `make_mut` and `get_mut` can be used safely and
/// `get_mut_unchecked` can be reasoned about.  Note: can we combine the strong
/// count and the ledger's local increments to say something useful about
/// uniqueness?
#[derive(Debug)]
pub struct Box<T> {
    ptr: Pointer<T>,
}

impl<T> Box<T> {
    /// Put `value` on the heap
    ///
    /// # Examples
    ///
    /// ```
    ///# use crossbeam_epoch::arc::Box;
    /// let mut b = Box::new(123);
    /// *b += 1;
    /// ```
    pub fn new(value: T) -> Self {
        // Safety: the value returned by `alloc::boxed::Box::into_raw` is
        // "properly aligned and non-null"
        Self {
            ptr: Pointer::new(value),
        }
    }
}

impl<T: Clone> Clone for Box<T> {
    /// Deep copy preserves uniqueness and mutability
    fn clone(&self) -> Self {
        Self::new(self.ptr.deref().clone())
    }
}

impl<T: Default> Default for Box<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> Deref for Box<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.ptr.deref()
    }
}

impl<T> DerefMut for Box<T> {
    fn deref_mut(&mut self) -> &mut T {
        // Safety: Box guarantees uniqueness
        unsafe { self.ptr.deref_mut() }
    }
}

impl<T> Drop for Box<T> {
    fn drop(&mut self) {
        // Safety: Box guarantees uniqueness
        unsafe {
            self.ptr.drop_data();
            self.ptr.dealloc();
        }
    }
}

unsafe impl<T: Sync + Send> Send for Box<T> {}

unsafe impl<T: Sync + Send> Sync for Box<T> {}

/// Atomically reference-counted heap allocated `T` suitable for lock-free
/// atomic operations on the pointer itself
///
/// Compare with `alloc::sync::Arc`
///
#[derive(Debug)]
pub struct Arc<T> {
    ptr: Pointer<T>,
}

impl<T> Arc<T> {
    /// Make a `Weak` pointer
    ///
    /// ```
    ///# use crossbeam_epoch::arc::Arc;
    /// let arc = Arc::new(123);
    /// let weak = Arc::downgrade(&arc);
    /// ```
    pub fn downgrade(this: &Self) -> Weak<T> {
        this.ptr.downgrade()
    }

    /// Provides unsafe mutable access
    ///
    /// # Safety
    ///
    /// This is even more dangerous than for core::sync::Arc as the reference
    /// count is only eventually consistent.
    ///
    /// About the only safe use for this is with a freshly created Arc that has
    /// not yet been cloned or through an atomic.  Prefer Box for that use case.
    pub unsafe fn get_mut_unchecked(this: &mut Self) -> &mut T {
        this.ptr.deref_mut()
    }

    /// New reference counted heap allocated T
    ///
    /// ```
    ///# use crossbeam_epoch::arc::Arc;
    /// let a = Arc::new(123);
    /// ```
    pub fn new(value: T) -> Self {
        Self {
            ptr: Pointer::new(value),
        }
    }

    /// Pointer equality (Eq checks value equality)
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.ptr == other.ptr
    }

    /// Get the approximate number of strong references
    ///
    /// # Safety
    ///
    /// Safe to call but impossible to use the result in any safe way.  Only for
    /// debugging.  Deferred reference counting makes it even less useful than
    /// `core::sync::Arc::strong_count`.
    ///
    pub fn strong_count(this: &Self) -> usize {
        let old = this.ptr.strong_count();
        debug_assert_ne!(old, 0);
        old
    }

    /// Get the approximate number of weak references
    ///
    /// # Safety
    ///
    /// Safe to call but the result is immediately stale and can't be used in
    /// any safe way without additional knowledge of all referents.  Only for
    /// debugging.
    pub fn weak_count(this: &Self) -> usize {
        this.ptr.weak_count()
    }

    /// Load (rather than clone) a cheap, limited-lifetime pointer
    pub fn load<'a>(this: &'a Self) -> BorrowedArc<'a, T> {
        BorrowedArc {
            ptr: this.ptr,
            _marker: PhantomData,
        }
    }
}

impl<T: Clone> Arc<T> {
    /// Make a deep copy and return in a mutable wrapper
    pub fn make_mut(this: &Self) -> Box<T> {
        Box::new(this.ptr.deref().clone())
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Arc<T> {
        self.ptr.defer_incr_strong();
        Arc { ptr: self.ptr }
    }
}

impl<T: Default> Default for Arc<T> {
    fn default() -> Self {
        Arc::new(T::default())
    }
}

impl<T> Deref for Arc<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.ptr.deref()
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        self.ptr.defer_decr_strong();
    }
}

impl<T> From<T> for Arc<T> {
    fn from(value: T) -> Self {
        Arc::new(value)
    }
}

impl<T> From<Box<T>> for Arc<T> {
    fn from(x: Box<T>) -> Self {
        let Box { ptr, .. } = x;
        Self { ptr }
    }
}

unsafe impl<T: Sync + Send> Send for Arc<T> {}

unsafe impl<T: Sync + Send> Sync for Arc<T> {}

/// Replace `core::sync::Weak`
#[derive(Debug)]
pub struct Weak<T> {
    ptr: Pointer<T>,
}

impl<T> Weak<T> {
    /// Pointer equality
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }

    /// Approximate strong count.  Non-zero-ness is meaningful.
    ///
    /// # Safety
    ///
    /// Safe to call, hard to use safely.  Loaded with relaxed ordering.
    pub fn strong_count(&self) -> usize {
        self.ptr.strong_count()
    }

    /// `Weak::upgrade` still works with deferred reference counting, but it
    /// does make delayed collection visible to the rest of the program.  In the
    /// use case of a weak cache this seems fine.
    ///
    /// # Example
    ///
    /// ```
    ///# use crossbeam_epoch::arc::Arc;
    /// let arc = Arc::new(123);
    /// let weak = Arc::downgrade(&arc);
    /// let strong = weak.upgrade();
    /// assert_eq!(arc, strong.unwrap());
    /// ```
    pub fn upgrade(&self) -> Option<Arc<T>> {
        self.ptr.upgrade()
    }

    /// Approximate weak count
    ///
    /// Loaded with relaxed ordering, use fences if desired
    pub fn weak_count(&self) -> usize {
        self.ptr.weak_count()
    }
}

impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        self.ptr.defer_incr_weak();
        Weak { ptr: self.ptr }
    }
}

impl<T> Drop for Weak<T> {
    fn drop(&mut self) {
        self.ptr.defer_decr_weak()
    }
}

/// A pointer protected by a lifetime guarantee, either an `epoch::Guard`
/// guaranteeing that that pointer will not be dropped, or a borrow from an
/// ordinary `ArcLike` object
#[derive(Debug)]
pub struct BorrowedArc<'a, T> {
    ptr: Pointer<T>,
    _marker: PhantomData<&'a ()>,
}

impl<'a, T> BorrowedArc<'a, T> {
    /// Clone as weak
    pub fn downgrade(this: &Self) -> Weak<T> {
        this.ptr.downgrade()
    }

    /// Load a cheap limited lifetime Arc
    pub fn load<'b>(&'b self) -> BorrowedArc<'b, T> {
        BorrowedArc {
            ptr: self.ptr,
            _marker: PhantomData,
        }
    }

    /// Pointer equality (Eq checks value equality)
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        this.ptr == other.ptr
    }

    /// Strong reference count
    pub fn strong_count(this: &Self) -> usize {
        this.ptr.strong_count()
    }

    /// Weak reference count
    pub fn weak_count(this: &Self) -> usize {
        this.ptr.weak_count()
    }
}

impl<'a, T> Clone for BorrowedArc<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T> Copy for BorrowedArc<'a, T> {}

impl<'a, T> Deref for BorrowedArc<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.ptr.deref()
    }
}

/// A scoped non-owning Weak pointer
#[derive(Debug)]
pub struct BorrowedWeak<'a, T> {
    ptr: Pointer<T>,
    _marker: PhantomData<&'a ()>,
}

impl<'a, T> BorrowedWeak<'a, T> {
    /// Pointer equality
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }

    /// Approximate strong count.  Non-zero-ness is meaningful.
    ///
    /// # Safety
    ///
    /// Safe to call, hard to use safely.  Loaded with relaxed ordering.
    pub fn strong_count(&self) -> usize {
        self.ptr.strong_count()
    }

    /// Try and produce a strong pointer
    pub fn upgrade(&self) -> Option<Arc<T>> {
        self.ptr.upgrade()
    }

    /// Approximate weak count
    ///
    /// Loaded with relaxed ordering, use fences if desired
    pub fn weak_count(&self) -> usize {
        self.ptr.weak_count()
    }
}

impl<'a, T> Clone for BorrowedWeak<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T> Copy for BorrowedWeak<'a, T> {}

/// Provide reference counting coalescence and deferment
///
/// Rather than directly manipulate the reference count, we write increments and
/// decrements into a thread_local ledger.  When a Guard is dropped or repinned,
/// any increments that have not been cancelled by decrements are applied
/// atomically to the reference counts; any decrements that have not been
/// cancelled by increments are packaged up and handed off to the garbage
/// collector to execute after the current epoch.
///
/// Common patterns like load-deref-compare-exchange produce short-lived
/// pointers that do not outlive the Guard; we never need to touch the reference
/// counts of these objects and the Ledger automates the bookkeeping to prove
/// this, with non-atomic thread_local lookups that are hopefully cheaper than
/// contended atomic reference counts.
///
/// The struct holds
/// - A map from type-erased pointers to outstanding counts and function
///   pointers which can recover the types and apply the counts
/// - The same for weak counts
/// - A container of garbage found during the current collection, which we must
///   collect immediately (else long chains of sole ownership would only be
///   drained one node per collection)
/// - A flag to tell destructors that a collection is happening and to put their
///   decrements directly into the garbage container
pub(crate) struct Ledger {
    // Whatever mapping structure is chosen here will determine performance; we
    // make the consciously naive choice to use the general purpose HashMap
    // while developing rather than prematurely optimize
    pending_strong: RefCell<HashMap<usize, (isize, fn(usize, isize))>>,
    // Can we use the same structure for this? usize -> (isize, isize, fn(usize,
    // isize, isize))
    pending_weak: RefCell<HashMap<usize, (isize, fn(usize, isize))>>,
    garbage: RefCell<Vec<(usize, fn(usize))>>,
    is_collecting: Cell<bool>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            pending_strong: RefCell::default(),
            pending_weak: RefCell::default(),
            garbage: RefCell::default(),
            is_collecting: Cell::new(false),
        }
    }
}

fn with_ledger<R, F: FnMut(&Ledger, bool) -> R>(mut f: F) -> R {
    with_handle(|handle| f(unsafe { &(*handle.local).ledger }, handle.is_pinned()))
}

impl Ledger {
    /// Logically increment the strong reference count
    /// - Never if we can find an outstanding decrement to cancel out
    /// - Immediately if the thread is not pinned
    /// - Otherwise at the end of the current pin
    ///   - Hoping that a decrement will cancel us first
    fn defer_incr_strong<T>(&self, ptr: Pointer<T>, is_pinned: bool) {
        match self.pending_strong.borrow_mut().entry(ptr.as_usize()) {
            Occupied(mut entry) => {
                // This pointer already has pending operations
                entry.get_mut().0 += 1;
                if entry.get_mut().0 == 0 {
                    // One decrement was outstanding and we cancelled it
                    entry.remove();
                }
            }
            Vacant(entry) => {
                if is_pinned {
                    // Register to be incremented at end of pin
                    entry.insert((1, |data, n| {
                        debug_assert!(n > 0);
                        unsafe { Pointer::<T>::from_usize(data) }.add_strong(n as usize);
                    }));
                } else {
                    // Increment right now
                    ptr.add_strong(1);
                }
            }
        }
    }

    /// Logically increment the weak reference count
    fn defer_incr_weak<T>(&self, ptr: Pointer<T>, is_pinned: bool) {
        match self.pending_weak.borrow_mut().entry(ptr.as_usize()) {
            Occupied(mut entry) => {
                entry.get_mut().0 += 1;
                if entry.get_mut().0 == 0 {
                    entry.remove();
                }
            }
            Vacant(entry) => {
                if is_pinned {
                    entry.insert((1, |data, n| {
                        debug_assert!(n > 0);
                        unsafe { Pointer::<T>::from_usize(data) }.add_weak(n as usize);
                    }));
                } else {
                    ptr.add_weak(1);
                }
            }
        }
    }

    /// Logically decrement the strong reference count
    /// - Register to be decremented in the future
    /// - Cancel an existing increment
    /// - If a collection triggered the decrement, register for immediate
    ///   collection
    fn defer_decr_strong<T>(&self, ptr: Pointer<T>) {
        if self.is_collecting.get() {
            self.garbage
                .borrow_mut()
                .push((ptr.as_usize(), |data| unsafe {
                    Pointer::<T>::from_usize(data).sub_strong(1)
                }))
        } else {
            match self.pending_strong.borrow_mut().entry(ptr.as_usize()) {
                Occupied(mut entry) => {
                    entry.get_mut().0 -= 1;
                    if entry.get_mut().0 == 0 {
                        entry.remove();
                    }
                }
                Vacant(entry) => {
                    entry.insert((-1, |data, n| {
                        debug_assert!(n < 0);
                        unsafe { Pointer::<T>::from_usize(data).sub_strong((-n) as usize) }
                    }));
                }
            }
        }
    }

    fn defer_decr_weak<T>(&self, ptr: Pointer<T>) {
        if self.is_collecting.get() {
            self.garbage
                .borrow_mut()
                .push((ptr.as_usize(), |data| unsafe {
                    Pointer::<T>::from_usize(data).sub_weak(1)
                }))
        } else {
            match self.pending_weak.borrow_mut().entry(ptr.as_usize()) {
                Occupied(mut entry) => {
                    entry.get_mut().0 -= 1;
                    if entry.get_mut().0 == 0 {
                        entry.remove();
                    }
                }
                Vacant(entry) => {
                    entry.insert((-1, |data, n| {
                        debug_assert!(n < 0);
                        unsafe { Pointer::<T>::from_usize(data).sub_weak((-n) as usize) }
                    }));
                }
            }
        }
    }
    /// Immediately apply all outstanding increments on this thread; called just
    /// before we unpin so the increments are visible before the collector runs
    /// any decrements from the current epoch
    fn apply_pending_increments(&self) {
        // predicate with side effects is ok?
        self.pending_strong
            .borrow_mut()
            .retain(|&ptr, &mut (count, apply)| {
                (count < 0) || {
                    apply(ptr, count);
                    false
                }
            });
        self.pending_weak
            .borrow_mut()
            .retain(|&ptr, &mut (count, apply)| {
                (count < 0) || {
                    apply(ptr, count);
                    false
                }
            });
    }

    /// Immediately apply all pending decrements; this is run by the garbage
    /// collector after the epoch has moved on
    fn apply_pending_decrements(
        mut pending_strong: HashMap<usize, (isize, fn(usize, isize))>,
        mut pending_weak: HashMap<usize, (isize, fn(usize, isize))>,
    ) {
        // This is run by the garbage collector on an arbitrary thread.  First
        // step is to reconnect with that thread's ArcManager
        with_handle(move |this| {
            let ledger = unsafe { &(*this.local).ledger };
            // set a flag to communicate that all Drops happening on this
            // thread are due to garbage collection
            let old = ledger.is_collecting.replace(true);
            debug_assert!(!old);
            // apply pending decrements
            for (ptr, (count, apply)) in pending_strong.drain() {
                debug_assert!(count < 0);
                apply(ptr, count)
            }
            for (ptr, (count, apply)) in pending_weak.drain() {
                debug_assert!(count < 0);
                apply(ptr, count)
            }
            loop {
                let x;
                {
                    x = ledger.garbage.borrow_mut().pop();
                }
                if let Some((ptr, apply)) = x {
                    apply(ptr);
                } else {
                    break;
                }
            }
            // finished; reset flag
            let old = ledger.is_collecting.replace(false);
            debug_assert!(old);
        })
    }
}

pub(crate) trait UnpinObserver {
    fn will_unpin(self: &Self, guard: &Guard);
}

impl UnpinObserver for Local {
    fn will_unpin(self: &Self, guard: &Guard) {
        let ledger = &self.ledger;
        ledger.apply_pending_increments();
        // todo: we need to reuse memory better here
        if !ledger.pending_strong.borrow().is_empty() || !ledger.pending_weak.borrow().is_empty() {
            let pending_strong = ledger.pending_strong.replace(HashMap::default());
            let pending_weak = ledger.pending_weak.replace(HashMap::default());
            unsafe {
                guard.defer_unchecked(move || {
                    Ledger::apply_pending_decrements(pending_strong, pending_weak);
                })
            }
        }
    }
}

/// Types implementing `ArcLike` can be round-tripped via usize but don't
/// necessarily support reference counting.  All of `Box`, `Arc`, `Weak`,
/// `BorrowedArc`, `BorrowedWeak`, `Option<T: ArcLike>` and `(T: ArcLike, usize)
/// are `ArcLike`.
///
/// The pointer-as-usize may be null or have tag bits.  Unlike
/// `alloc::sync::Arc::into_raw` we are dealing with a pointer to the control
/// block, not the payload `T` at some offset inside the control block
pub trait ArcLike {
    /// The type of the payload
    type Type;

    /// Get a pointer to Inner<T>, possibly null, possibly with a tag in the
    /// alignment bits, as a usize
    fn as_usize(this: &Self) -> usize;

    /// Reconstruct Self from this *const Inner<T> as usize, which may be
    /// null or tagged
    ///
    /// # Safety
    ///
    /// Argument must have resulted from a call to into_usize
    unsafe fn from_usize(data: usize) -> Self;
}

/// Trait providing a way to turn borrowed things into owning things
pub trait Redeemable {
    /// Owning version of this type (identity except for BorrowedArc)
    type Owned;

    /// Upgrade to owning version (clone except for BorrowedArc)
    ///
    /// Calling this `to_owned` collides with `core::borrow::Borrow`
    fn redeem(this: &Self) -> Self::Owned;
}

impl<T> ArcLike for Box<T> {
    type Type = T;

    fn as_usize(this: &Self) -> usize {
        this.ptr.as_usize()
    }

    unsafe fn from_usize(data: usize) -> Self {
        Self {
            ptr: Pointer::from_usize(data),
        }
    }
}

impl<T> ArcLike for Arc<T> {
    type Type = T;

    fn as_usize(this: &Self) -> usize {
        this.ptr.as_usize()
    }

    unsafe fn from_usize(data: usize) -> Self {
        Self {
            ptr: Pointer::from_usize(data),
        }
    }
}

impl<'a, T> ArcLike for BorrowedArc<'a, T> {
    type Type = T;

    fn as_usize(this: &Self) -> usize {
        this.ptr.as_usize()
    }

    unsafe fn from_usize(data: usize) -> Self {
        Self {
            ptr: Pointer::from_usize(data),
            _marker: PhantomData,
        }
    }
}

impl<'a, T> Redeemable for BorrowedArc<'a, T> {
    type Owned = Arc<T>;
    fn redeem(this: &Self) -> Self::Owned {
        this.ptr.defer_incr_strong();
        Arc { ptr: this.ptr }
    }
}

impl<T> ArcLike for Weak<T> {
    type Type = T;

    fn as_usize(this: &Self) -> usize {
        this.ptr.as_usize()
    }

    unsafe fn from_usize(data: usize) -> Self {
        Self {
            ptr: Pointer::from_usize(data),
        }
    }
}

impl<'a, T> ArcLike for BorrowedWeak<'a, T> {
    type Type = T;

    fn as_usize(this: &Self) -> usize {
        this.ptr.as_usize()
    }

    unsafe fn from_usize(data: usize) -> Self {
        Self {
            ptr: Pointer::from_usize(data),
            _marker: PhantomData,
        }
    }
}

impl<'a, T> Redeemable for BorrowedWeak<'a, T> {
    type Owned = Arc<T>;
    fn redeem(this: &Self) -> Self::Owned {
        this.ptr.defer_incr_strong();
        Arc { ptr: this.ptr }
    }
}

/// Lets us use Option<Arc<T>>
impl<T: ArcLike> ArcLike for Option<T> {
    type Type = T::Type;

    fn as_usize(this: &Self) -> usize {
        match this {
            None => 0,
            Some(this) => T::as_usize(this),
        }
    }

    unsafe fn from_usize(data: usize) -> Self {
        match data {
            0 => None,
            data => Some(T::from_usize(data)),
        }
    }
}

impl<T> Redeemable for Option<T>
where
    T: Redeemable,
{
    type Owned = Option<T::Owned>;
    fn redeem(this: &Self) -> Self::Owned {
        this.as_ref().map(|x| T::redeem(x))
    }
}

/// Lets us write tagged ArcLikes
impl<T: ArcLike> ArcLike for (T, usize) {
    type Type = T::Type;

    fn as_usize(this: &Self) -> usize {
        T::as_usize(&this.0) | (this.1 & tag_mask::<Inner<Self::Type>>())
    }

    unsafe fn from_usize(data: usize) -> Self {
        (
            T::from_usize(data & ptr_mask::<Inner<Self::Type>>()),
            data & tag_mask::<Inner<Self::Type>>(),
        )
    }
}

impl<T> Redeemable for (T, usize)
where
    T: Redeemable,
{
    type Owned = (T::Owned, usize);
    fn redeem(this: &Self) -> Self::Owned {
        (T::redeem(&this.0), this.1)
    }
}

/// Marks a nullable pointer; i.e. `Option<_>` and the derived `(Option<_>,
/// usize)`
pub trait Nullable {
    /// Return the null value for this atomic reference counted pointer type
    fn null() -> Self;
}

impl<T: ArcLike> Nullable for Option<T> {
    fn null() -> Self {
        None
    }
}

impl<T: ArcLike + Nullable> Nullable for (T, usize) {
    fn null() -> Self {
        (T::null(), 0)
    }
}

/// Marks types that may be borrowed and provides the type they may be borrowed
/// as.  The trait is generic on all lifetimes as a workaround for the lack of
/// generic associated types; types are bounded with the `T: for<'a>
/// Borrowable<'a>` syntax.
///
/// Only `Box<_>` is not `Borrowable`
pub trait Borrowable<'a> {
    /// The type that `&'a Self` may be borrowed as, and the return type of
    /// `load` and failed `compare_exchange` on an atomic with a `&'a Guard`.
    type Type: ArcLike;
}

impl<'a, T> Borrowable<'a> for Arc<T> {
    type Type = BorrowedArc<'a, T>;
}

impl<'a, 'b, T> Borrowable<'a> for BorrowedArc<'b, T> {
    type Type = BorrowedArc<'b, T>;
}

impl<'a, T> Borrowable<'a> for Weak<T> {
    type Type = BorrowedWeak<'a, T>;
}

impl<'a, 'b, T> Borrowable<'a> for BorrowedWeak<'b, T> {
    type Type = BorrowedWeak<'b, T>;
}

impl<'a, U> Borrowable<'a> for Option<U>
where
    U: Borrowable<'a>,
{
    type Type = Option<U::Type>;
}

impl<'a, U> Borrowable<'a> for (U, usize)
where
    U: Borrowable<'a>,
{
    type Type = (U::Type, usize);
}

/// Trait for argument conversions
pub trait ArcInto<T> {
    /// Convert the maybe-null, maybe-tagged pointers into common format, with
    /// necessary ownership manipulations
    fn into_usize(this: Self) -> usize;
}

/// Complementary trait for argument conversions
pub trait ArcFrom<T> {}
impl<T, U> ArcFrom<T> for U where T: ArcInto<U> {}

// The implementation of this trait has a lot of duplication that I can't work
// out how to avoid without trait specialization
//
// Roughly speaking, owning to owning conversions just forget the source to
// transfer ownership to the target
//
// Borrowed to owning conversions perform a (deferred) increment
//
// Optional and tagged forms call down to their base types
//
// Where tags are involved we also require ArcLike to access the pointee type
// and thus mask the tag bits appropriately

impl<T> ArcInto<Box<T>> for Box<T> {
    fn into_usize(this: Self) -> usize {
        let data = Box::as_usize(&this);
        forget(this);
        data
    }
}
impl<T> ArcInto<Arc<T>> for Box<T> {
    fn into_usize(this: Self) -> usize {
        let data = Box::as_usize(&this);
        forget(this);
        data
    }
}
impl<T, U> ArcInto<Option<U>> for Box<T>
where
    Box<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}
impl<T, U> ArcInto<(U, usize)> for Box<T>
where
    Box<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<T> ArcInto<Arc<T>> for Arc<T> {
    fn into_usize(this: Self) -> usize {
        let data = Arc::as_usize(&this);
        forget(this);
        data
    }
}
impl<T, U> ArcInto<Option<U>> for Arc<T>
where
    Arc<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}
impl<T, U> ArcInto<(U, usize)> for Arc<T>
where
    Arc<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<'a, T> ArcInto<BorrowedArc<'a, T>> for BorrowedArc<'a, T> {
    fn into_usize(this: Self) -> usize {
        BorrowedArc::as_usize(&this)
    }
}

impl<'a, T> ArcInto<Arc<T>> for BorrowedArc<'a, T> {
    fn into_usize(this: Self) -> usize {
        this.ptr.defer_incr_strong();
        BorrowedArc::as_usize(&this)
    }
}
impl<'a, T, U> ArcInto<Option<U>> for BorrowedArc<'a, T>
where
    BorrowedArc<'a, T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}
impl<'a, T, U> ArcInto<(U, usize)> for BorrowedArc<'a, T>
where
    BorrowedArc<'a, T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<T> ArcInto<Weak<T>> for Weak<T> {
    fn into_usize(this: Self) -> usize {
        let data = Weak::as_usize(&this);
        forget(this);
        data
    }
}
impl<T, U> ArcInto<Option<U>> for Weak<T>
where
    Weak<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}
impl<T, U> ArcInto<(U, usize)> for Weak<T>
where
    Weak<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<'a, T> ArcInto<BorrowedWeak<'a, T>> for BorrowedWeak<'a, T> {
    fn into_usize(this: Self) -> usize {
        BorrowedWeak::as_usize(&this)
    }
}

impl<'a, T> ArcInto<Weak<T>> for BorrowedWeak<'a, T> {
    fn into_usize(this: Self) -> usize {
        this.ptr.defer_incr_weak();
        BorrowedWeak::as_usize(&this)
    }
}
impl<'a, T, U> ArcInto<Option<U>> for BorrowedWeak<'a, T>
where
    BorrowedWeak<'a, T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<'a, T, U> ArcInto<(U, usize)> for BorrowedWeak<'a, T>
where
    BorrowedWeak<'a, T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<T, U> ArcInto<Option<U>> for Option<T>
where
    T: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        this.map_or(0, |x| <T as ArcInto<U>>::into_usize(x))
    }
}

impl<T, U> ArcInto<(U, usize)> for Option<T>
where
    Option<T>: ArcInto<U>,
{
    fn into_usize(this: Self) -> usize {
        <Self as ArcInto<U>>::into_usize(this)
    }
}

impl<T, U> ArcInto<(U, usize)> for (T, usize)
where
    T: ArcInto<U>,
    U: ArcLike,
{
    fn into_usize(this: Self) -> usize {
        <T as ArcInto<U>>::into_usize(this.0) | (this.1 & tag_mask::<Inner<U::Type>>())
    }
}

// Tagged nulls
impl<U> ArcInto<Option<U>> for usize
where
    U: ArcLike,
{
    fn into_usize(this: Self) -> usize {
        this & tag_mask::<Inner<U::Type>>()
    }
}

impl<U> ArcInto<(Option<U>, usize)> for usize
where
    U: ArcLike,
{
    fn into_usize(this: Self) -> usize {
        this & tag_mask::<Inner<U::Type>>()
    }
}

/// Result of a failed compare-exchange operation
///
/// Returns the desired value back to the caller, as well as the unexpected
/// current value.  For the weak variant, the current value may be equal to the
/// expected value.
///
/// ```
///# use core::sync::atomic::Ordering;
///# use crossbeam_epoch::arc::{Arc, Atomic, Box};
///# use crossbeam_epoch::pin;
/// let guard = pin();
/// let a = Atomic::<Option<Arc<i32>>>::new(Some(Arc::new(123)));
/// let b = unsafe {
///     a.compare_exchange(Option::<Arc<i32>>::None, Box::new(456), Ordering::Acquire, &guard)
/// };
/// match b {
///     Ok(_) => {
///         assert!(false)
///     },
///     Err(err) => {
///         assert_eq!(*err.current.unwrap(), 123);
///         assert_eq!(*err.desired, 456);
///     }
/// }
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct CompareExchangeError<Current, Desired> {
    /// The current value of the atomic
    pub current: Current,

    /// The value that failed to be swapped into the atomic
    pub desired: Desired,
}

/// A lock-free atomic reference counted pointer
/// - optionally nullable
/// - optionally tagged
/// - optionally !Clone
/// - coalesced and deferred reference counting
/// - fine-grained synchronization control (and attendant unsafety)
///
/// Compare Mutex<(Option<Arc<T>>, usize)>
///
/// # Examples
///
/// # Safety
///
/// Memory order safety is vital and intrinsically non-local.  All atomic
/// operations take arbitrary memory orders and are marked unsafe; this allows
/// efficient synchronization but places the burden of safety onto the user.
///
/// The interface prevents trivial mistakes like improper null values.
///
#[derive(Debug)]
pub struct Atomic<T: ArcLike> {
    data: AtomicUsize,
    _marker: PhantomData<T>,
}

/// Atomic<Box<T>> can be exchanged but not loaded or compared
///
impl<T: ArcLike> Atomic<T> {
    /// Construct a new Atomic holding the supplied pointer
    ///
    /// # Examples
    ///
    /// ```
    ///# use crossbeam_epoch::arc::{Arc, Atomic, Box};
    /// let a = Atomic::<Arc<i32>>::new(Arc::new(123)); // shared
    /// let b = Atomic::<Box<i32>>::new(Box::new(123)); // mutable
    /// let c = Atomic::<Option<Arc<i32>>>::new(Some(Arc::new(123))); // nullable
    /// let c = Atomic::<(Arc<i32>, usize)>::new((Arc::new(123), 1)); // tagged
    /// ```
    ///
    pub fn new<U>(x: U) -> Self
    where
        U: ArcInto<T>,
    {
        Self {
            data: AtomicUsize::new(U::into_usize(x)),
            _marker: PhantomData,
        }
    }

    /// Consume an atomic and return the stored pointer
    ///
    /// # Examples
    ///
    /// ```
    ///# use crossbeam_epoch::arc::{Arc, Atomic};
    /// let a = 123;
    /// let b = Arc::new(a);
    /// let c = Atomic::<Arc<i32>>::new(b);
    /// assert_eq!(a, *c.into_inner());
    /// ```
    pub fn into_inner(self) -> T {
        let mut y = self;
        let x = unsafe { T::from_usize(*(y.data.get_mut())) };
        forget(y);
        x
    }

    /// # Safety
    ///
    /// See module.  Swap is unusual in that it doesn't require a guard as it
    /// makes no immediate or deferred changes to the reference counts of any
    /// objects
    pub unsafe fn swap<New: ArcInto<T>>(&self, new: New, order: Ordering) -> T {
        T::from_usize(self.data.swap(New::into_usize(new), order))
    }

    /// # Safety
    ///
    /// See module
    pub unsafe fn store<New: ArcInto<T>>(&self, new: New, order: Ordering) {
        self.swap(new, order);
    }
}

impl<T: ArcLike + Nullable> Atomic<T> {
    /// Take the stored pointer, replacing it with null
    ///
    /// # Safety
    ///
    /// Memory ordering established here or by other means must be sufficient
    /// for the use made of the return type
    ///
    /// ```
    ///# use core::sync::atomic::Ordering;
    ///# use crossbeam_epoch::arc::{Arc, Atomic};
    /// let a = Atomic::<Option<Arc<i32>>>::new(Some(Arc::new(123)));
    /// unsafe {
    ///     let b = a.take(Ordering::Acquire);
    /// }
    /// ```
    pub unsafe fn take(&self, order: Ordering) -> T {
        T::from_usize(self.data.swap(0, order))
    }

    /// Produce a new null Atomic<T>
    pub fn null() -> Self {
        Self {
            data: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }
}

impl<T> Atomic<T>
where
    T: ArcLike + for<'a> Borrowable<'a>,
{
    /// Load the stored pointer.
    ///
    /// # Performance
    ///
    /// The classic problem in concurrent reference counting is to atomically
    /// load the pointer and increment the reference count behind it.  This
    /// implementation relies on crossbeam's epoch system.  All threads agree to
    /// defer reference count decrements on pointers loaded from atomics until
    /// after all threads have moved on from the current epoch.  As a
    /// consequence, reference count increments performed by a thread can also
    /// be deferred until just before the thread exits its pinned state.
    ///
    /// We can use this flexibility to never manipulate the atomic reference
    /// counts of short-lived pointers that are dropped before the thread
    /// unpins.  Instead we note logical increments and decrements in a
    /// thread-local `Ledger`, cancelling out paired increments and decrements
    /// as the opportunity arises.  Only pointers that are passed out of the
    /// pinned region need to have their increments applied (just before
    /// unpinning) or decrements applied (sometime in a future garbage
    /// collection).  This is preferable because the atomic operations are
    /// relatively expensive, particularly the decrements which must be
    /// performed with `Release` ordering.
    ///
    /// # Safety
    ///
    /// Memory ordering established here or by other means must be sufficient
    /// for the use made of the return type.  `Acquire` is often correct but
    /// `Relaxed` may be sufficient for some uses.
    ///
    /// ```
    ///# use core::sync::atomic::Ordering;
    ///# use crossbeam_epoch::{pin, arc::{Arc, Atomic, BorrowedArc, Redeemable}};
    /// let atom = Atomic::<Arc<i32>>::new(Arc::new(123));
    /// let owned;
    /// unsafe {
    ///     let guard = pin();
    ///     let borrowed = atom.load(Ordering::Acquire, &guard);
    ///     assert_eq!(BorrowedArc::strong_count(&borrowed), 1);
    ///     owned = Redeemable::redeem(&borrowed);
    ///     assert_eq!(Arc::strong_count(&owned), 1);
    /// }   // <-- guard drop triggers increment
    /// assert_eq!(Arc::strong_count(&owned), 2);
    /// assert_eq!(*owned, 123);
    /// ```
    pub unsafe fn load<'g>(
        &self,
        order: Ordering,
        _guard: &'g Guard,
    ) -> <T as Borrowable<'g>>::Type {
        let old = self.data.load(order);
        <T as Borrowable<'g>>::Type::from_usize(old)
    }

    /// Load the stored pointer with consume-memory-ordering and schedule a
    /// reference count increment
    ///
    /// # Safety
    ///
    /// Memory ordering established here or by other means must be sufficient
    /// for the use made of the return type
    pub unsafe fn load_consume<'g>(&self, _guard: &'g Guard) -> <T as Borrowable<'g>>::Type {
        let old = self.data.load_consume();
        <T as Borrowable<'g>>::Type::from_usize(old)
    }

    /// Exchange the stored value only if it is still the value we expect.
    ///
    /// When called in loops, as is typical, the `expected` argument will be of
    /// the `Borrowed` type returned by `load` and failed `compare_exchange`.
    /// However, the `expected` argument is very liberal in what it accepts; an
    /// Arc can be compared any combination of box, arc, weak, borrowed, null
    /// and tagged, because the comparison is performed on the basic (tagged)
    /// representation they all share, and the comparison argument is never
    /// required to have or transfer ownership.  For example, literal integers
    /// `n` are interpreted as `(None, n)` and can be used when a (tagged) null
    /// value is expected.  
    ///
    /// The `desired` argument must be able to convert into a `T` when the
    /// exchange succeeds; if it is a `Borrowed` type (and non-null), a deferred
    /// increment is scheduled upon success.  If the exchange fails, `desired`
    /// is returned to the caller in `Err`.
    ///
    /// # Result
    ///
    /// - On success, the previous value, owned
    /// - On failure, the current value, borrowed, and the desired value,
    ///   unchanged
    ///
    /// # Performance
    ///
    /// - The corresponding `AtomicUsize::compare_exchange` operation is called
    ///   with the provided memory orderings.
    /// - Expected is dropped
    ///   - if borrowed (common) or null this is a no-op
    ///   - if owning (rare?) and non-null, a deferred decrement will be
    ///     scheduled
    /// - On success, and if desired is borrowed and non-null, a deferred
    ///   increment will be scheduled.
    ///
    /// # Safety
    ///
    /// Memory ordering established here or by other means must be sufficient
    /// for the use made of the return type
    ///
    /// # Examples
    ///
    /// ```
    ///# use core::sync::atomic::Ordering;
    ///# use crossbeam_epoch::{pin, arc::{Arc, Atomic, Box}};
    /// let a = Atomic::<Arc<i32>>::new(Arc::new(123));
    /// let mut desired = Box::new(456);
    /// let guard = pin();
    /// let mut expected = unsafe { a.load(Ordering::Relaxed, &guard) };
    /// let old;
    /// loop {
    ///     match unsafe { a.compare_exchange_weak(expected, desired,
    /// (Ordering::AcqRel, Ordering::Relaxed), &guard) } {
    ///         Ok(ok) => {
    ///             old = ok;
    ///             break;
    ///         },
    ///         Err(err) => {
    ///             expected = err.current;
    ///             desired = err.desired;
    ///         }
    ///     }
    /// }
    /// assert_eq!(*old, 123);
    /// ```
    #[allow(clippy::type_complexity)]
    pub unsafe fn compare_exchange<
        'g,
        Expected: ArcLike,
        Desired: ArcLike + ArcInto<T>,
        Order: CompareAndSetOrdering,
    >(
        &self,
        expected: Expected,
        desired: Desired,
        order: Order,
        _guard: &'g Guard,
    ) -> Result<T, CompareExchangeError<<T as Borrowable<'g>>::Type, Desired>> {
        let desired_as_usize = Desired::as_usize(&desired);
        let expected_as_usize = Expected::as_usize(&expected);
        match self.data.compare_exchange(
            expected_as_usize,
            desired_as_usize,
            order.success(),
            order.failure(),
        ) {
            Ok(old) => {
                Desired::into_usize(desired); // <-- desired transfers ownership
                Ok(T::from_usize(old))
            }
            Err(current) => Err(CompareExchangeError {
                current: <T as Borrowable<'g>>::Type::from_usize(current),
                desired: desired,
            }),
        }
        // <-- expected is dropped
    }

    /// See `compare_exchange`
    ///
    /// # Safety
    ///
    /// See module
    #[allow(clippy::type_complexity)]
    pub unsafe fn compare_exchange_weak<
        'g,
        Expected: ArcLike,
        Desired: ArcLike + ArcInto<T>,
        Order: CompareAndSetOrdering,
    >(
        &self,
        expected: Expected,
        desired: Desired,
        order: Order,
        _guard: &'g Guard,
    ) -> Result<T, CompareExchangeError<<T as Borrowable<'g>>::Type, Desired>> {
        let desired_as_usize = Desired::as_usize(&desired);
        let expected_as_usize = Expected::as_usize(&expected);
        match self.data.compare_exchange_weak(
            expected_as_usize,
            desired_as_usize,
            order.success(),
            order.failure(),
        ) {
            Ok(old) => {
                Desired::into_usize(desired); // <-- desired transfers ownership
                Ok(T::from_usize(old))
            }
            Err(current) => Err(CompareExchangeError {
                current: <T as Borrowable<'g>>::Type::from_usize(current),
                desired: desired,
            }),
        }
        // <-- expected is dropped
    }
}

impl<T> Atomic<(T, usize)>
where
    T: ArcLike + for<'a> Borrowable<'a>,
{
    /// # Safety
    ///
    /// See module
    pub unsafe fn fetch_and<'g>(
        &self,
        tag: usize,
        order: Ordering,
        _guard: &'g Guard,
    ) -> (<T as Borrowable<'g>>::Type, usize) {
        let old = self
            .data
            .fetch_and(tag | !tag_mask::<Inner<<T as ArcLike>::Type>>(), order);
        <(<T as Borrowable<'g>>::Type, usize)>::from_usize(old)
    }

    /// Atomically apply a tag to a pointer
    ///
    /// # Safety
    ///
    /// See module
    ///
    /// ```
    ///# use core::sync::atomic::Ordering;
    ///# use crossbeam_epoch::{pin, arc::{Arc, Atomic}};
    /// let a = Atomic::<(Arc<i32>, usize)>::new((Arc::new(123), 1));
    /// let guard = pin();
    /// let old = unsafe { a.fetch_or(2, Ordering::Relaxed, &guard) };
    /// assert_eq!(old.1, 1);
    /// let current = unsafe { a.load(Ordering::Relaxed, &guard) };
    /// assert_eq!(*current.0, 123);
    /// assert_eq!(current.1, 3);
    /// ```
    pub unsafe fn fetch_or<'g>(
        &self,
        tag: usize,
        order: Ordering,
        _guard: &'g Guard,
    ) -> (<T as Borrowable<'g>>::Type, usize) {
        let old = self
            .data
            .fetch_or(tag & tag_mask::<Inner<<T as ArcLike>::Type>>(), order);
        <(<T as Borrowable<'g>>::Type, usize)>::from_usize(old)
    }

    /// # Safety
    ///
    /// See module
    pub unsafe fn fetch_xor<'g>(
        &self,
        tag: usize,
        order: Ordering,
        _guard: &'g Guard,
    ) -> (<T as Borrowable<'g>>::Type, usize) {
        let old = self
            .data
            .fetch_xor(tag & tag_mask::<Inner<<T as ArcLike>::Type>>(), order);
        <(<T as Borrowable<'g>>::Type, usize)>::from_usize(old)
    }
}

impl<T: ArcLike + Default + ArcInto<T>> Default for Atomic<T> {
    fn default() -> Self {
        Atomic::new(T::default())
    }
}

impl<T: ArcLike> Drop for Atomic<T> {
    fn drop(&mut self) {
        unsafe { T::from_usize(*self.data.get_mut()) };
    }
}

impl<T: ArcLike, U: ArcLike + ArcInto<T>> From<U> for Atomic<T> {
    fn from(x: U) -> Self {
        Self {
            data: AtomicUsize::new(U::into_usize(x)),
            _marker: PhantomData,
        }
    }
}

mod test {

    //use super::{Arc, ArcLike, Atomic, Redeemable, Box};
    //use crate::pin;
    //use core::sync::atomic::{AtomicIsize, Ordering};
}

impl<T> Eq for Pointer<T> {}

impl<T> PartialEq for Pointer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<T> AsMut<T> for Box<T> {
    fn as_mut(&mut self) -> &mut T {
        // Safety: Box invariants guarantee uniqueness
        unsafe { self.ptr.deref_mut() }
    }
}

impl<T> AsRef<T> for Box<T> {
    fn as_ref(&self) -> &T {
        self.ptr.deref()
    }
}

impl<T: Eq> Eq for Box<T> {}

impl<T> From<T> for Box<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: Ord> Ord for Box<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        Ord::cmp(self.ptr.deref(), other.ptr.deref())
    }
}

impl<T: PartialEq> PartialEq for Box<T> {
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(self.ptr.deref(), other.ptr.deref())
    }
}

impl<T: PartialOrd> PartialOrd for Box<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        PartialOrd::partial_cmp(self.ptr.deref(), other.ptr.deref())
    }
}

impl<T> AsRef<T> for Arc<T> {
    fn as_ref(&self) -> &T {
        self.ptr.deref()
    }
}

impl<T: Eq> Eq for Arc<T> {}

impl<T: Ord> Ord for Arc<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<T: PartialEq> PartialEq for Arc<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: PartialOrd> PartialOrd for Arc<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
}

impl<'a, T> AsRef<T> for BorrowedArc<'a, T> {
    fn as_ref(&self) -> &T {
        self.ptr.deref()
    }
}

impl<'a, T: Eq> Eq for BorrowedArc<'a, T> {}

impl<'a, T: Ord> Ord for BorrowedArc<'a, T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<'a, T: PartialEq> PartialEq for BorrowedArc<'a, T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<'a, T: PartialOrd> PartialOrd for BorrowedArc<'a, T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
}
