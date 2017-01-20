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
/// - a `usize` tag value that can fit into the unused bits of the pointer:
/// `tag < std::mem::align_of::<T>()`
#[derive(Debug)]
pub struct TaggedAtomic<T> {
    ptr: atomic::AtomicUsize,
    _marker: PhantomData<*const T>,
}

unsafe impl<T: Sync> Send for TaggedAtomic<T> {}
unsafe impl<T: Sync> Sync for TaggedAtomic<T> {}

/// Returns 2 to the power of the number of unused bits
/// in a pointer to `T`. Any tag placed on such a pointer
/// must be strictly less than this value.
fn tag_ceil<T>() -> usize {
    1 << mem::align_of::<T>().trailing_zeros()
}

/// Verifies that the tag can fit into the unused bits of a pointer to `T`.
fn guard_tag<T>(tag: usize) {
    assert!(tag < tag_ceil::<T>(),
            "tag too large to fit into unused bits of pointer in TaggedAtomic: {} >= {}", tag, tag_ceil::<T>());
}

/// Retrieves the original pointer and the tag from a packed value.
fn unpack_tag<T>(val: usize) -> (*mut T, usize) {
    let ptr = (val & !(tag_ceil::<T>() - 1)) as *mut T;
    let tag =  val &  (tag_ceil::<T>() - 1);
    (ptr, tag)
}

fn opt_shared_into_usize<T>(ptr: Option<Shared<T>>, tag: usize) -> usize {
    guard_tag::<T>(tag);
    let raw = ptr.as_ref().map(Shared::as_raw).unwrap_or(ptr::null_mut());
    raw as usize | tag
}

fn opt_owned_as_usize<T>(ptr: &Option<Owned<T>>, tag: usize) -> usize {
    guard_tag::<T>(tag);
    let raw = ptr.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut());
    raw as usize | tag
}

fn opt_owned_into_usize<T>(ptr: Option<Owned<T>>, tag: usize) -> usize {
    guard_tag::<T>(tag);
    let raw = ptr.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut());
    mem::forget(ptr);
    raw as usize | tag
}

impl<T> TaggedAtomic<T> {
    /// Create a new, null tagged atomic pointer with a tag value of 0.
    #[cfg(feature = "nightly")]
    pub const fn zero() -> TaggedAtomic<T> {
        TaggedAtomic {
            ptr: atomic::AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Create a new, null tagged atomic pointer with a tag value of 0.
    #[cfg(not(feature = "nightly"))]
    pub fn zero() -> TaggedAtomic<T> {
        TaggedAtomic {
            ptr: atomic::AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Create a new, null tagged atomic pointer with the given tag value.
    pub fn null(tag: usize) -> TaggedAtomic<T> {
        guard_tag::<T>(tag);
        TaggedAtomic {
            ptr: atomic::AtomicUsize::new(tag),
            _marker: PhantomData,
        }
    }

    /// Create a new tagged atomic pointer.
    pub fn new(data: T, tag: usize) -> TaggedAtomic<T> {
        guard_tag::<T>(tag);
        TaggedAtomic {
            ptr: atomic::AtomicUsize::new(
                Box::into_raw(Box::new(data)) as usize | tag
            ),
            _marker: PhantomData,
        }
    }

    /// Create a new tagged atomic pointer from the given raw pointer.
    /// This is unsafe because the pointer must be either null or pointing
    /// to valid memory, otherwise it will lead to undefined behaviour.
    ///
    /// # Panics
    ///
    /// Panics if `tag >= mem::align_of::<T>()`.
    pub unsafe fn from_raw(ptr: *mut T, tag: usize) -> TaggedAtomic<T> {
        guard_tag::<T>(tag);
        TaggedAtomic {
            ptr: atomic::AtomicUsize::new(
                ptr as usize | tag
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
    /// lifetime tied to the guard, and the value of the tag.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Release` or `AcqRel`.
    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> (Option<Shared<'a, T>>, usize) {
        let (ptr, tag) = unpack_tag(self.ptr.load(ord));
        unsafe { (Shared::from_raw(ptr), tag) }
    }

    /// Do an atomic store with the given memory ordering.
    ///
    /// Transfers ownership of the given `Owned` pointer, if any. Since no
    /// lifetime information is acquired, no `Guard` value is needed.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel` or if `tag >= mem::align_of::<T>()`.
    pub fn store(&self, val: Option<Owned<T>>, tag: usize, ord: Ordering) {
        self.ptr.store(opt_owned_into_usize(val, tag), ord)
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
    /// Panics if `ord` is `Acquire` or `AcqRel` or if `tag >= mem::align_of::<T>()`.
    pub fn store_and_ref<'a>(&self, val: Owned<T>, tag: usize, ord: Ordering, _: &'a Guard)
                             -> Shared<'a, T>
    {
        unsafe {
            let shared = Shared::from_owned(val);
            self.store_shared(Some(shared), tag, ord);
            shared
        }
    }

    /// Do an atomic store of a `Shared` pointer and a tag value
    /// with the given memory ordering.
    ///
    /// This operation does not require a guard, because it does not yield any
    /// new information about the lifetime of a pointer.
    ///
    /// # Panics
    ///
    /// Panics if `ord` is `Acquire` or `AcqRel` or if `tag >= mem::align_of::<T>()`.
    pub fn store_shared(&self, val: Option<Shared<T>>, tag: usize, ord: Ordering) {
        self.ptr.store(opt_shared_into_usize(val, tag), ord)
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
    /// Panics if either `old_tag` or `new_tag` is `>= mem::align_of::<T>()`.
    pub fn cas(&self,
               old: Option<Shared<T>>, old_tag: usize,
               new: Option<Owned<T>>, new_tag: usize,
               ord: Ordering
              ) -> Result<(), Option<Owned<T>>>
    {
        if self.ptr.compare_and_swap(opt_shared_into_usize(old, old_tag),
                                     opt_owned_as_usize(&new, new_tag),
                                     ord) == opt_shared_into_usize(old, old_tag)
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
    /// Panics if either `old_tag` or `new_tag` is `>= mem::align_of::<T>()`.
    pub fn cas_and_ref<'a>(&self,
                           old: Option<Shared<T>>, old_tag: usize,
                           new: Owned<T>, new_tag: usize,
                           ord: Ordering,
                           _: &'a Guard
                          ) -> Result<Shared<'a, T>, Owned<T>>
    {
        guard_tag::<T>(new_tag);
        if self.ptr.compare_and_swap(
               opt_shared_into_usize(old, old_tag),
               new.as_raw() as usize | new_tag,
               ord
           ) == opt_shared_into_usize(old, old_tag)
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
    /// Panics if either `old_tag` or `new_tag` is `>= mem::align_of::<T>()`.
    pub fn cas_shared(&self,
                      old: Option<Shared<T>>, old_tag: usize,
                      new: Option<Shared<T>>, new_tag: usize,
                      ord: Ordering
                     ) -> bool
    {
        self.ptr.compare_and_swap(opt_shared_into_usize(old, old_tag),
                                  opt_shared_into_usize(new, new_tag),
                                  ord) == opt_shared_into_usize(old, old_tag)
    }

    /// Do an atomic swap with an `Owned` pointer in the new pair with the given memory ordering.
    ///
    /// # Panics
    ///
    /// Panics if `new_tag >= mem::align_of::<T>()`.
    pub fn swap<'a>(&self, new: Option<Owned<T>>, new_tag: usize, ord: Ordering, _: &'a Guard)
                    -> (Option<Shared<'a, T>>, usize) {
        let (ptr, tag) = unpack_tag(self.ptr.swap(opt_owned_into_usize(new, new_tag), ord));
        unsafe { (Shared::from_raw(ptr), tag) }
    }

    /// Do an atomic swap with a `Shared` pointer in the new pair with the given memory ordering.
    ///
    /// # Panics
    ///
    /// Panics if `new_tag >= mem::align_of::<T>()`.
    pub fn swap_shared<'a>(&self,
                           new: Option<Shared<T>>, new_tag: usize,
                           ord: Ordering,
                           _: &'a Guard
                          ) -> (Option<Shared<'a, T>>, usize) {
        let (ptr, tag) = unpack_tag(self.ptr.swap(opt_shared_into_usize(new, new_tag), ord));
        unsafe { (Shared::from_raw(ptr), tag) }
    }

    /// Perform a bitwise or on the current tag and the argument `tag` and set the new tag to
    /// the result. Returns the previous value of the tag.
    ///
    /// # Panics
    ///
    /// Panics if `tag >= mem::align_of::<T>()`.
    pub fn fetch_or(&self, tag: usize, ord: Ordering) -> usize {
        guard_tag::<T>(tag);
        self.ptr.fetch_or(tag, ord)
    }

    /// Perform a bitwise and on the current tag and the argument `tag` and set the new tag to
    /// the result. Returns the previous value of the tag.
    ///
    /// # Panics
    ///
    /// Panics if `tag >= mem::align_of::<T>()`.
    pub fn fetch_and(&self, tag: usize, ord: Ordering) -> usize {
        guard_tag::<T>(tag);
        self.ptr.fetch_and(tag | !(tag_ceil::<T>() - 1), ord)
    }

    /// Perform a bitwise xor on the current tag and the argument `tag` and set the new tag to
    /// the result. Returns the previous value of the tag.
    ///
    /// # Panics
    ///
    /// Panics if `tag >= mem::align_of::<T>()`.
    pub fn fetch_xor(&self, tag: usize, ord: Ordering) -> usize {
        guard_tag::<T>(tag);
        self.ptr.fetch_xor(tag, ord)
    }
}
