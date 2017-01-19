use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, Ordering};

use super::{Owned, Shared, Guard};

/// Provides atomic access to a `(*const T, usize)` pair.
///
/// The pair consists of:
///
/// - a (nullable) pointer of type `T`, interfacing with the `Owned` and `Shared` types
///
/// - a `usize` marker value that can fit into the unused bits of the pointer:
/// `mark < std::mem::align_of::<T>()`
#[derive(Debug)]
pub struct MarkableAtomic<T> {
    ptr: atomic::AtomicUsize,
    _marker: PhantomData<*const T>,
}

unsafe impl<T: Sync> Send for MarkableAtomic<T> {}
unsafe impl<T: Sync> Sync for MarkableAtomic<T> {}

/// Verifies that the mark can fit into the unused bits of a pointer to `T`.
fn guard_mark<T>(mark: usize) {
    assert!(mark < mem::align_of::<T>(),
            "mark too large to fit into unused bits of pointer in MarkableAtomic: {} >= {}", mark, mem::align_of::<T>());
}

/// Retrieves the original pointer and the mark from a packed value.
fn unpack_mark<T>(val: usize) -> (*mut T, usize) {
    let ptr = (val & !(mem::align_of::<T>() - 1)) as *mut T;
    let mark = val &  (mem::align_of::<T>() - 1);
    (ptr, mark)
}

fn opt_shared_into_usize<T>(ptr: Option<Shared<T>>, mark: usize) -> usize {
    guard_mark::<T>(mark);
    let raw = ptr.as_ref().map(Shared::as_raw).unwrap_or(ptr::null_mut());
    raw as usize | mark
}

fn opt_owned_as_usize<T>(ptr: &Option<Owned<T>>, mark: usize) -> usize {
    guard_mark::<T>(mark);
    let raw = ptr.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut());
    raw as usize | mark
}

fn opt_owned_into_usize<T>(ptr: Option<Owned<T>>, mark: usize) -> usize {
    guard_mark::<T>(mark);
    let raw = ptr.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut());
    mem::forget(ptr);
    raw as usize | mark
}

impl<T> MarkableAtomic<T> {
    /// Create a new, null markable atomic pointer with a marker value of 0.
    #[cfg(feature = "nightly")]
    pub const fn zero() -> MarkableAtomic<T> {
        MarkableAtomic {
            ptr: atomic::AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Create a new, null markable atomic pointer with a marker value of 0.
    #[cfg(not(feature = "nightly"))]
    pub fn zero() -> MarkableAtomic<T> {
        MarkableAtomic {
            ptr: atomic::AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Create a new, null markable atomic pointer with the given marker value.
    pub fn null(mark: usize) -> MarkableAtomic<T> {
        guard_mark::<T>(mark);
        MarkableAtomic {
            ptr: atomic::AtomicUsize::new(mark),
            _marker: PhantomData,
        }
    }

    /// Create a new markable atomic pointer.
    pub fn new(data: T, mark: usize) -> MarkableAtomic<T> {
        guard_mark::<T>(mark);
        MarkableAtomic {
            ptr: atomic::AtomicUsize::new(
                Box::into_raw(Box::new(data)) as usize | mark
            ),
            _marker: PhantomData,
        }
    }

    /// Create a new markable atomic pointer from the given raw pointer.
    /// This is unsafe because the pointer must be either null or pointing
    /// to valid memory, otherwise it will lead to undefined behaviour.
    ///
    /// # Panics
    ///
    /// Panics if `mark >= mem::align_of::<T>()`.
    pub unsafe fn from_raw(ptr: *mut T, mark: usize) -> MarkableAtomic<T> {
        guard_mark::<T>(mark);
        MarkableAtomic {
            ptr: atomic::AtomicUsize::new(
                ptr as usize | mark
            ),
            _marker: PhantomData,
        }
    }

    /// Do an atomic load with the given memory ordering.
    ///
    /// In order to perform the load, we must pass in a borrow of a
    /// `Guard`. This is a way of guaranteeing that the thread has pinned the
    /// epoch for the entire lifetime `'a`. In return, you get an optional
    /// `Shared` pointer back (`None` if the `Atomic` is currently null), with
    /// lifetime tied to the guard, and the value of the marker.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Release` or `AcqRel`.
    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> (Option<Shared<'a, T>>, usize) {
        let (ptr, mark) = unpack_mark(self.ptr.load(ord));
        unsafe { (Shared::from_raw(ptr), mark) }
    }

    /// Do an atomic store with the given memory ordering.
    ///
    /// Transfers ownership of the given `Owned` pointer, if any. Since no
    /// lifetime information is acquired, no `Guard` value is needed.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel` or if `mark >= mem::align_of::<T>()`.
    pub fn store(&self, val: Option<Owned<T>>, mark: usize, ord: Ordering) {
        self.ptr.store(opt_owned_into_usize(val, mark), ord)
    }

    /// Do an atomic store with the given memory ordering, immediately yielding
    /// a shared reference to the pointer that was stored.
    ///
    /// Transfers ownership of the given `Owned` pointer, yielding a `Shared`
    /// reference to it. Since the reference is valid only for the curent epoch,
    /// it's lifetime is tied to a `Guard` value.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel` or if `mark >= mem::align_of::<T>()`.
    pub fn store_and_ref<'a>(&self, val: Owned<T>, mark: usize, ord: Ordering, _: &'a Guard)
                             -> Shared<'a, T>
    {
        unsafe {
            let shared = Shared::from_owned(val);
            self.store_shared(Some(shared), mark, ord);
            shared
        }
    }

    /// Do an atomic store of a `Shared` pointer and a marker value
    /// with the given memory ordering.
    ///
    /// This operation does not require a guard, because it does not yield any
    /// new information about the lifetime of a pointer.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel` or if `mark >= mem::align_of::<T>()`.
    pub fn store_shared(&self, val: Option<Shared<T>>, mark: usize, ord: Ordering) {
        self.ptr.store(opt_shared_into_usize(val, mark), ord)
    }

    /// Do a compare-and-set on the pair, from a `Shared` to an `Owned` pointer in the pair
    /// with the given memory ordering.
    ///
    /// As with `store`, this operation does not require a guard; it produces no new
    /// lifetime information. The `Result` indicates whether the CAS succeeded; if
    /// not, ownership of the `new` pointer is returned to the caller.
    ///
    /// # Panics
    ///
    /// Panics if either `old_mark` or `new_mark` is `>= mem::align_of::<T>()`.
    pub fn cas(&self,
               old: Option<Shared<T>>, old_mark: usize,
               new: Option<Owned<T>>, new_mark: usize,
               ord: Ordering
              ) -> Result<(), Option<Owned<T>>>
    {
        if self.ptr.compare_and_swap(opt_shared_into_usize(old, old_mark),
                                     opt_owned_as_usize(&new, new_mark),
                                     ord) == opt_shared_into_usize(old, old_mark)
        {
            mem::forget(new);
            Ok(())
        } else {
            Err(new)
        }
    }

    /// Do a compare-and-set on the pair, from a `Shared` to an `Owned` pointer in the pair
    /// with the given memory ordering, immediatley acquiring a new `Shared` reference to
    /// the previously-owned pointer if successful.
    ///
    /// This operation is analogous to `store_and_ref`.
    ///
    /// # Panics
    ///
    /// Panics if either `old_mark` or `new_mark` is `>= mem::align_of::<T>()`.
    pub fn cas_and_ref<'a>(&self,
                           old: Option<Shared<T>>, old_mark: usize,
                           new: Owned<T>, new_mark: usize,
                           ord: Ordering,
                           _: &'a Guard
                          ) -> Result<Shared<'a, T>, Owned<T>>
    {
        guard_mark::<T>(new_mark);
        if self.ptr.compare_and_swap(
               opt_shared_into_usize(old, old_mark),
               new.as_raw() as usize | new_mark,
               ord
           ) == opt_shared_into_usize(old, old_mark)
        {
            Ok(unsafe { Shared::from_owned(new) })
        } else {
            Err(new)
        }
    }

    /// Do a compare-and-set on the pair, from a `Shared` to another `Shared` pointer in the pair
    /// with the given memory ordering.
    ///
    /// The boolean return value is `true` when the CAS is successful.
    ///
    /// # Panics
    ///
    /// Panics if either `old_mark` or `new_mark` is `>= mem::align_of::<T>()`.
    pub fn cas_shared(&self,
                      old: Option<Shared<T>>, old_mark: usize,
                      new: Option<Shared<T>>, new_mark: usize,
                      ord: Ordering
                     ) -> bool
    {
        self.ptr.compare_and_swap(opt_shared_into_usize(old, old_mark),
                                  opt_shared_into_usize(new, new_mark),
                                  ord) == opt_shared_into_usize(old, old_mark)
    }

    /// Do an atomic swap with an `Owned` pointer in the new pair with the given memory ordering.
    ///
    /// # Panics
    ///
    /// Panics if `new_mark >= mem::align_of::<T>()`.
    pub fn swap<'a>(&self, new: Option<Owned<T>>, new_mark: usize, ord: Ordering, _: &'a Guard)
                    -> (Option<Shared<'a, T>>, usize) {
        let (ptr, mark) = unpack_mark(self.ptr.swap(opt_owned_into_usize(new, new_mark), ord));
        unsafe { (Shared::from_raw(ptr), mark) }
    }

    /// Do an atomic swap with a `Shared` pointer in the new pair with the given memory ordering.
    ///
    /// # Panics
    ///
    /// Panics if `new_mark >= mem::align_of::<T>()`.
    pub fn swap_shared<'a>(&self,
                           new: Option<Shared<T>>, new_mark: usize,
                           ord: Ordering,
                           _: &'a Guard
                          ) -> (Option<Shared<'a, T>>, usize) {
        let (ptr, mark) = unpack_mark(self.ptr.swap(opt_shared_into_usize(new, new_mark), ord));
        unsafe { (Shared::from_raw(ptr), mark) }
    }

    /// Perform a bitwise or on the current mark and the argument `mark` and set the new mark to
    /// the result. Returns the previous value of the mark.
    ///
    /// # Panics
    ///
    /// Panics if `mark >= mem::align_of::<T>()`.
    pub fn or_mark(&self, mark: usize, ord: Ordering) -> usize {
        guard_mark::<T>(mark);
        self.ptr.fetch_or(mark, ord)
    }

    /// Perform a bitwise and on the current mark and the argument `mark` and set the new mark to
    /// the result. Returns the previous value of the mark.
    ///
    /// # Panics
    ///
    /// Panics if `mark >= mem::align_of::<T>()`.
    pub fn and_mark(&self, mark: usize, ord: Ordering) -> usize {
        guard_mark::<T>(mark);
        self.ptr.fetch_and(mark, ord)
    }
}
