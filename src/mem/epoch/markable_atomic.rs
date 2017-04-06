use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, Ordering};

use super::{Owned, Shared, Guard};

/// Like a `(std::sync::atomic::AtomicPtr, bool)` pair.
///
/// Provides atomic access to a pair consisting of:
///
/// - a (nullable) pointer of type `T`, interfacing with the `Owned` and `Shared` types
///
/// - a boolean marker bit
#[derive(Debug)]
pub struct MarkableAtomic<T> {
    val: atomic::AtomicUsize,
    _marker: PhantomData<*const T>,
}

unsafe impl<T: Sync> Send for MarkableAtomic<T> {}
unsafe impl<T: Sync> Sync for MarkableAtomic<T> {}

/// Tags an even usize with a boolean marker bit.
fn tag_val(p: usize, b: bool) -> usize {
    debug_assert!(p as usize & 1 == 0);
    p | b as usize
}

/// Retrieves the original value and the boolean marker bit from a tagged usize.
fn untag_val(t: usize) -> (usize, bool) {
    let mark = t & 1;
    (t - mark, mark == 1)
}

fn opt_shared_into_usize<T>(val: Option<Shared<T>>) -> usize {
    val.as_ref().map(Shared::as_raw).unwrap_or(ptr::null_mut()) as usize
}

fn opt_owned_as_usize<T>(val: &Option<Owned<T>>) -> usize {
    val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut()) as usize
}

fn opt_owned_into_usize<T>(val: Option<Owned<T>>) -> usize {
    let ptr = val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut());
    mem::forget(val);
    ptr as usize
}


impl<T> MarkableAtomic<T> {
    /// Create a new, null, markable atomic pointer with the given marker value.
    #[cfg(feature = "nightly")]
    pub /*const*/ fn null(mark: bool) -> MarkableAtomic<T> {
        debug_assert!(mem::align_of::<T>() >= 2);

        MarkableAtomic {
            val: atomic::AtomicUsize::new(tag_val(0, mark)),
            _marker: PhantomData,
        }
    }

    /// Create a new, null, markable atomic pointer with the given marker value.
    #[cfg(not(feature = "nightly"))]
    pub fn null(mark: bool) -> MarkableAtomic<T> {
        debug_assert!(mem::align_of::<T>() >= 2);

        MarkableAtomic {
            val: atomic::AtomicUsize::new(tag_val(0, mark)),
            _marker: PhantomData,
        }
    }

    /// Create a new markable atomic pointer.
    pub fn new(data: T, mark: bool) -> MarkableAtomic<T> {
        debug_assert!(mem::align_of::<T>() >= 2);

        MarkableAtomic {
            val: atomic::AtomicUsize::new(tag_val(Box::into_raw(Box::new(data)) as usize, mark)),
            _marker: PhantomData,
        }
    }

    /// Unsafely create a new markable atomic pointer from the given value.
    pub unsafe fn from_ptr(ptr: *mut T, mark: bool) -> MarkableAtomic<T> {
        debug_assert!(mem::align_of::<T>() >= 2);

        MarkableAtomic {
            val: atomic::AtomicUsize::new(tag_val(ptr as usize, mark)),
            _marker: PhantomData,
        }
    }

    /// Do an atomic load with the given memory ordering.
    ///
    /// In order to perform the load, we must pass in a borrow of a
    /// `Guard`. This is a way of guaranteeing that the thread has pinned the
    /// epoch for the entire lifetime `'a`. In return, you get an optional
    /// `Shared` pointer back (`None` if the `Atomic` is currently null), with
    /// lifetime tied to the guard, and the value of the marker bit.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Release` or `AcqRel`.
    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> (Option<Shared<'a, T>>, bool) {
        let p = untag_val(self.val.load(ord));
        (unsafe { Shared::from_raw(p.0 as *mut _) }, p.1)
    }

    /// Do an atomic store with the given memory ordering.
    ///
    /// Transfers ownership of the given `Owned` pointer, if any. Since no
    /// lifetime information is acquired, no `Guard` value is needed.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store(&self, new: Option<Owned<T>>, mark: bool, ord: Ordering) {
        self.val.store(tag_val(opt_owned_into_usize(new), mark), ord);
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
    pub fn store_and_ref<'a>(&self, new: Owned<T>, mark: bool, ord: Ordering, _: &'a Guard)
                             -> Shared<'a, T>
    {
        unsafe {
            let shared = Shared::from_owned(new);
            self.store_shared(Some(shared), mark, ord);
            shared
        }
    }

    /// Do an atomic store of a `Shared` pointer and a marker bit
    /// with the given memory ordering.
    ///
    /// This operation does not require a guard, because it does not yield any
    /// new information about the lifetime of a pointer.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel`.
    pub fn store_shared(&self, new: Option<Shared<T>>, mark: bool, ord: Ordering) {
        self.val.store(tag_val(opt_shared_into_usize(new), mark), ord);
    }

    /// Do a compare-and-set on the pair, from a `Shared` to an `Owned` pointer in the pair
    /// with the given memory ordering.
    ///
    /// As with `store`, this operation does not require a guard; it produces no new
    /// lifetime information. The `Result` indicates whether the CAS succeeded; if
    /// not, ownership of the `new` pointer is returned to the caller.
    pub fn cas(&self, old: Option<Shared<T>>, old_mark: bool, new: Option<Owned<T>>, new_mark: bool, ord: Ordering)
               -> Result<(), Option<Owned<T>>>
    {
        if self.val.compare_and_swap(tag_val(opt_shared_into_usize(old), old_mark),
                                     tag_val(opt_owned_as_usize(&new), new_mark),
                                     ord) == opt_shared_into_usize(old)
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
    pub fn cas_and_ref<'a>(&self, old: Option<Shared<T>>, old_mark: bool, new: Owned<T>, new_mark: bool, ord: Ordering, _: &'a Guard)
                           -> Result<Shared<'a, T>, Owned<T>>
    {
        if self.val.compare_and_swap(tag_val(opt_shared_into_usize(old), old_mark),
                                     tag_val(new.as_raw() as usize, new_mark),
                                     ord) == opt_shared_into_usize(old)
        {
            Ok(unsafe { Shared::from_owned(new) } )
        } else {
            Err(new)
        }
    }

    /// Do a compare-and-set on the pair, from a `Shared` to another `Shared` pointer in the pair
    /// with the given memory ordering.
    ///
    /// The boolean return value is `true` when the CAS is successful.
    pub fn cas_shared(&self, old: Option<Shared<T>>, old_mark: bool, new: Option<Shared<T>>, new_mark: bool, ord: Ordering)
                      -> bool
    {
        self.val.compare_and_swap(tag_val(opt_shared_into_usize(old), old_mark),
                                  tag_val(opt_shared_into_usize(new), new_mark),
                                  ord) == opt_shared_into_usize(old)
    }

    /// Do an atomic swap with an `Owned` pointer in the new pair with the given memory ordering.
    pub fn swap<'a>(&self, new: Option<Owned<T>>, new_mark: bool, ord: Ordering, _: &'a Guard)
                    -> (Option<Shared<'a, T>>, bool)
    {
        let prev = untag_val(self.val.swap(tag_val(opt_owned_into_usize(new), new_mark), ord));
        (unsafe { Shared::from_raw(prev.0 as *mut _) }, prev.1 )
    }

    /// Do an atomic swap with a `Shared` pointer in the new pair with the given memory ordering.
    pub fn swap_shared<'a>(&self, new: Option<Shared<T>>, new_mark: bool, ord: Ordering, _: &'a Guard)
                           -> (Option<Shared<'a, T>>, bool)
    {
        let prev = untag_val(self.val.swap(tag_val(opt_shared_into_usize(new), new_mark), ord));
        (unsafe { Shared::from_raw(prev.0 as *mut _) }, prev.1 )
    }
}
