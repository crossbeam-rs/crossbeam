use std::sync::atomic::{self, Ordering};
use std::{marker, ptr, mem};

use super::{Shared, Guard};

/// Convert `Option<Shared<T>>` into `*mut T`.
///
/// `None` maps to the null pointer.
#[inline]
fn opt_shared_into_raw<T>(val: Option<Shared<T>>) -> *mut T {
    // If `None`, return the null pointer.
    val.map(|p| p.as_raw()).unwrap_or(ptr::null_mut())
}

/// Convert `Option<Box<T>>` into `*mut T`.
///
/// `None` maps to the null pointer.
#[inline]
fn opt_box_into_raw<T>(val: Option<Box<T>>) -> *mut T {
    // If `None`, return the null pointer.
    val.map(Box::into_raw).unwrap_or(ptr::null_mut())
}

/// Convert `&Option<Box<T>>` into `*mut T`.
///
/// `&None` maps to the null pointer.
#[inline]
fn opt_box_as_raw<T>(val: &mut Option<Box<T>>) -> *mut T {
    // If `None`, return the null pointer.
    val.as_mut().map(|x| &mut **x as *mut T).unwrap_or(ptr::null_mut())
}

/// Like `std::sync::atomic::AtomicPtr`.
///
/// Provides atomic access to a (nullable) pointer of type `T`, interfacing with
/// the  `Shared` type.
#[derive(Debug)]
pub struct Atomic<T> {
    /// The inner atomic pointer.
    ptr: atomic::AtomicPtr<T>,
    /// A phantom marker to fix certain compiler bugs.
    // TODO
    _marker: marker::PhantomData<*const ()>,
}

impl<T> Atomic<T> {
    /// Create a new, null atomic pointer.
    #[cfg(feature = "nightly")]
    pub const fn null() -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(ptr::null_mut()),
            _marker: marker::PhantomData,
        }
    }

    /// Create a new, null atomic pointer.
    #[cfg(not(feature = "nightly"))]
    pub fn null() -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(ptr::null_mut()),
            _marker: marker::PhantomData,
        }
    }

    /// Create a new atomic pointer.
    ///
    /// This allocates `data` on the heap and creates an atomic pointer to it.
    pub fn new(data: T) -> Atomic<T> {
        Atomic {
            // As the data has to be stored _somewhere_, we have to allocate it to the heap.
            ptr: atomic::AtomicPtr::new(Box::into_raw(Box::new(data))),
            _marker: marker::PhantomData,
        }
    }

    /// Do an atomic load with a given memory ordering.
    ///
    /// In order to perform the load, we must pass in a borrow of a `Guard`. This is a way of
    /// guaranteeing that the thread has pinned the epoch for the entire lifetime `'a`. In return,
    /// you get an optional `Shared` pointer back (`None` if the `Atomic` is currently null), with
    /// lifetime tied to the guard.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Release` or `AcqRel`.
    pub fn load<'a>(&self, ord: Ordering, guard: &'a Guard) -> Option<Shared<'a, T>> {
        // Load the pointer.
        unsafe { self.ptr.load(ord).as_ref() }
            // Construct the `Shared` pointer.
            .map(|x| guard.new_shared(x))
    }

    /// Do an atomic store with a given memory ordering.
    ///
    /// This transfers ownership of the given `Box` pointer, if any. Since no lifetime
    /// information is acquired, no `Guard` value is needed.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store(&self, val: Option<Box<T>>, ord: Ordering) {
        self.ptr.store(opt_box_into_raw(val), ord)
    }

    /// Do an atomic store with the given memory ordering and get a shared reference.
    ///
    /// This transfers ownership of the given `Box` pointer, yielding a `Shared` reference to it.
    /// Since the reference is valid only for the curent epoch, it's lifetime is tied to a `Guard`
    /// value.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store_and_ref<'a>(&self, val: Box<T>, ord: Ordering, guard: &'a Guard) -> Shared<'a, T> {
        let shared = guard.new_shared(&*val);
        self.store_shared(Some(shared), ord);
        shared
    }

    /// Do an atomic store of a `Shared` pointer with a given memory ordering.
    ///
    /// This operation does not require a guard, because it does not yield any new information
    /// about the lifetime of a pointer.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store_shared(&self, val: Option<Shared<T>>, ord: Ordering) {
        self.ptr.store(opt_shared_into_raw(val), ord)
    }

    /// Atomically compare the value against `old` and swap it with `new` if matching.
    ///
    /// This is commonly refered to as 'CAS'. `self` is compared to `old`. If equal, `self` is set
    /// to `new` and `Ok(())` returned. Otherwise, `Err(actual_value)` is returned (with
    /// `actual_value` being the read value of `self`).
    ///
    /// All this is done atomically according to the atomic ordering specified in `ord`.
    pub fn compare_and_swap<'a>(&self, old: Option<Shared<T>>, new: Option<Shared<T>>, ord: Ordering, guard: &'a Guard)
        -> Result<(), Option<Shared<'a, T>>> {
        // Convert `old` into a raw pointer.
        let old = opt_shared_into_raw(old);
        // Compare it against `new`.
        let found = self.ptr.compare_and_swap(old, opt_shared_into_raw(new), ord);

        if found == old {
            // They matched, so the CAS succeeded.
            Ok(())
        } else {
            // They did not match, so we will return a shared pointer to the value.
            Err(unsafe { found.as_ref() }.map(|x| guard.new_shared(x)))
        }
    }

    /// Compare-and-set from a `Shared` to an `Box` pointer with a given memory ordering.
    ///
    /// As with `store`, this operation does not require a guard; it produces no new lifetime
    /// information. The `Result` indicates whether the CAS succeeded; if not, ownership of the
    /// `new` pointer is returned to the caller.
    pub fn compare_and_set(&self, old: Option<Shared<T>>, mut new: Option<Box<T>>, ord: Ordering)
        -> Result<(), Option<Box<T>>> {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old), opt_box_as_raw(&mut new), ord)
            == opt_shared_into_raw(old) {
            // This is crucial to avoid `new`'s destructor being called, which could cause an
            // dangling pointer to leak into safe API.
            mem::forget(new);
            Ok(())
        } else {
            // Hand back the ownership.
            Err(new)
        }
   }

    /// Compare-and-set from a `Shared` to an `Box` pointer with a given memory ordering and get
    /// a shared reference to it.
    ///
    /// This operation is analogous to `store_and_ref`.
    pub fn compare_and_set_ref<'a>(&self, old: Option<Shared<T>>, new: Box<T>, ord: Ordering, guard: &'a Guard)
        -> Result<Shared<'a, T>, Box<T>> {
        // FIXME: Ugly code.

        // Cast the box into a raw pointer.
        let new = Box::into_raw(new);

        if self.ptr.compare_and_swap(opt_shared_into_raw(old), new, ord)
            == opt_shared_into_raw(old) {
            // Create a `Shared` pointer.
            Ok(guard.new_shared(unsafe { &*new }))
        } else {
            Err(unsafe { Box::from_raw(new) })
        }
    }

    /// compare-and-set from a `Shared` to another `Shared` pointer with a given memory ordering.
    ///
    /// The boolean return value is `true` when the CAS is successful.
    pub fn compare_and_set_shared(&self, old: Option<Shared<T>>, new: Option<Shared<T>>, ord: Ordering)
        -> bool {
        self.ptr.compare_and_swap(opt_shared_into_raw(old), opt_shared_into_raw(new), ord)
            == opt_shared_into_raw(old)
    }

    /// Do an atomic swap with an `Box` pointer with the given memory ordering.
    pub fn swap<'a>(&self, new: Option<Box<T>>, ord: Ordering, guard: &'a Guard)
        -> Option<Shared<'a, T>> {
        unsafe {
            // Swap the inner.
            self.ptr.swap(opt_box_into_raw(new), ord).as_ref()
                // Construct the `Shared` pointer.
                .map(|x| guard.new_shared(&*x))
        }
    }

    /// Do an atomic swap with a `Shared` pointer with a given memory ordering.
    pub fn swap_shared<'a>(&self, new: Option<Shared<T>>, ord: Ordering, guard: &'a Guard)
        -> Option<Shared<'a, T>> {
        unsafe {
            // Swap the inner.
            self.ptr.swap(opt_shared_into_raw(new), ord).as_ref()
                // Construct the `Shared` pointer.
                .map(|x| guard.new_shared(&*x))
        }
    }
}

impl<T> Default for Atomic<T> {
    fn default() -> Atomic<T> {
        Atomic::null()
    }
}

unsafe impl<T: Sync> Send for Atomic<T> {}
unsafe impl<T: Sync> Sync for Atomic<T> {}
