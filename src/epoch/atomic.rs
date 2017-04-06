use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, Ordering};

use super::{Owned, Shared, Guard};

/// Like `std::sync::atomic::AtomicPtr`.
///
/// Provides atomic access to a (nullable) pointer of type `T`, interfacing with
/// the `Owned` and `Shared` types.
#[derive(Debug)]
pub struct Atomic<T> {
    /// The inner atomic pointer.
    ptr: atomic::AtomicPtr<T>,
    /// A phantom marker to fix certain compiler bugs.
    // FIXME
    _marker: PhantomData<*const ()>,
}

unsafe impl<T: Sync> Send for Atomic<T> {}
unsafe impl<T: Sync> Sync for Atomic<T> {}

/// Convert `Option<Shared<T>>` into `*mut T`.
///
/// `None` maps to the null pointer.
#[inline]
fn opt_shared_into_raw<T>(val: Option<Shared<T>>) -> *mut T {
    val.map(|p| p.as_raw()).unwrap_or(ptr::null_mut())
}

/// Convert `&Option<Owned<T>>` into `*mut T`.
///
/// `&None` maps to the null pointer.
#[inline]
fn opt_owned_as_raw<T>(val: &Option<Owned<T>>) -> *mut T {
    val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut())
}

/// Convert `Option<Owned<T>>` into `*mut T`.
///
/// `None` maps to the null pointer.
#[inline]
fn opt_owned_into_raw<T>(val: Option<Owned<T>>) -> *mut T {
    let ptr = val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut());
    mem::forget(val);
    ptr
}

impl<T> Atomic<T> {
    /// Create a new, null atomic pointer.
    #[cfg(feature = "nightly")]
    pub const fn null() -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(0 as *mut _),
            _marker: PhantomData,
        }
    }

    #[cfg(not(feature = "nightly"))]
    pub fn null() -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(0 as *mut _),
            _marker: PhantomData,
        }
    }

    /// Create a new atomic pointer
    pub fn new(data: T) -> Atomic<T> {
        Atomic {
            // As the data has to be stored _somewhere_, we have to allocate it to the heap.
            ptr: atomic::AtomicPtr::new(Box::into_raw(Box::new(data))),
            _marker: PhantomData,
        }
    }

    /// Do an atomic load with the given memory ordering.
    ///
    /// In order to perform the load, we must pass in a borrow of a
    /// `Guard`. This is a way of guaranteeing that the thread has pinned the
    /// epoch for the entire lifetime `'a`. In return, you get an optional
    /// `Shared` pointer back (`None` if the `Atomic` is currently null), with
    /// lifetime tied to the guard.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Release` or `AcqRel`.
    pub fn load<'a>(&self, ord: Ordering, guard: &'a Guard) -> Option<Shared<'a, T>> {
        Shared::from_raw(self.ptr.load(ord), guard)
    }

    /// Do an atomic store with the given memory ordering.
    ///
    /// Transfers ownership of the given `Owned` pointer, if any. Since no
    /// lifetime information is acquired, no `Guard` value is needed.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store(&self, val: Option<Owned<T>>, ord: Ordering) {
        self.ptr.store(opt_owned_into_raw(val), ord)
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
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store_ref<'a>(&self, val: Owned<T>, ord: Ordering, guard: &'a Guard)
                         -> Shared<'a, T> {
        let shared = Shared::from_owned(val, guard);
        self.store_shared(Some(shared), ord);
        shared
    }

    /// Do an atomic store of a `Shared` pointer with the given memory ordering.
    ///
    /// This operation does not require a guard, because it does not yield any
    /// new information about the lifetime of a pointer.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store_shared(&self, val: Option<Shared<T>>, ord: Ordering) {
        self.ptr.store(opt_shared_into_raw(val), ord)
    }

    /// Atomically compare the value against `old` and swap it with `new` if matching.
    ///
    /// This is commonly refered to as 'CAS'. `self` is compared to `old`.  If equal,
    /// `self` is set to `new` and `Ok(())` returned. Otherwise, `Err(actual_value)`
    /// is returned (with `actual_value` being the read value of `self`).
    ///
    /// All this is done atomically according to the atomic ordering specified in `ord`.
    pub fn compare_and_swap<'a>(&self, old: Option<Shared<T>>, new: Option<Shared<T>>, ord: Ordering, guard: &'a Guard)
        -> Result<(), Option<Shared<'a, T>>> {
        let old = opt_shared_into_raw(old);

        // Compare `old` against `new`.
        let found = self.ptr.compare_and_swap(old, opt_shared_into_raw(new), ord);
        if found == old {
            // They matched, so the CAS succeeded.
            Ok(())
        } else {
            // They did not match, so we will return a shared pointer to the value.
            Err(Shared::from_raw(found, guard))
        }
    }

    /// Compare-and-set from a `Shared` to an `Owned` pointer with the given memory
    /// ordering.
    ///
    /// As with `store`, this operation does not require a guard; it produces no new
    /// lifetime information. The `Result` indicates whether the CAS succeeded; if
    /// not, ownership of the `new` pointer is returned to the caller.
    pub fn compare_and_set(&self, old: Option<Shared<T>>, new: Option<Owned<T>>, ord: Ordering)
                           -> Result<(), Option<Owned<T>>> {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old),
                                     opt_owned_as_raw(&new),
                                     ord) == opt_shared_into_raw(old) {
            // This is crucial to avoid `new`'s destructor being called, which could cause an
            // dangling pointer to leak into safe API.
            mem::forget(new);
            Ok(())
        } else {
            // Hand back the ownership.
            Err(new)
        }
    }

    /// Compare-and-set from a `Shared` to an `Owned` pointer with the given memory
    /// ordering, immediatley acquiring a new `Shared` reference to the
    /// previously-owned pointer if successful.
    ///
    /// This operation is analogous to `store_ref`.
    pub fn compare_and_set_ref<'a>(&self, old: Option<Shared<T>>, new: Owned<T>,
                                   ord: Ordering, guard: &'a Guard)
                                   -> Result<Shared<'a, T>, Owned<T>> {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old), new.as_raw(), ord)
            == opt_shared_into_raw(old) {
            Ok(Shared::from_owned(new, guard))
        } else {
            Err(new)
        }
    }

    /// Compare-and-set from a `Shared` to another `Shared` pointer with the given
    /// memory ordering.
    ///
    /// The boolean return value is `true` when the CAS is successful.
    pub fn compare_and_set_shared(&self, old: Option<Shared<T>>, new: Option<Shared<T>>, ord: Ordering)
                                  -> bool {
        self.ptr.compare_and_swap(opt_shared_into_raw(old),
                                  opt_shared_into_raw(new),
                                  ord) == opt_shared_into_raw(old)
    }

    /// Do an atomic swap with an `Owned` pointer with the given memory ordering.
    pub fn swap<'a>(&self, new: Option<Owned<T>>, ord: Ordering, guard: &'a Guard)
                    -> Option<Shared<'a, T>> {
        Shared::from_raw(self.ptr.swap(opt_owned_into_raw(new), ord), guard)
    }

    /// Do an atomic swap with a `Shared` pointer with the given memory ordering.
    pub fn swap_shared<'a>(&self, new: Option<Shared<T>>, ord: Ordering, guard: &'a Guard)
                           -> Option<Shared<'a, T>> {
        Shared::from_raw(self.ptr.swap(opt_shared_into_raw(new), ord), guard)
    }
}
