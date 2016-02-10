//! Epoch-based memory management
//!
//! This module provides fast, easy to use memory management for lock free data
//! structures. It's inspired by [Keir Fraser's *epoch-based
//! reclamation*](https://www.cl.cam.ac.uk/techreports/UCAM-CL-TR-579.pdf).
//!
//! The basic problem this is solving is the fact that when one thread has
//! removed a node from a data structure, other threads may still have pointers
//! to that node (in the form of snapshots that will be validated through things
//! like compare-and-swap), so the memory cannot be immediately freed. Put differently:
//!
//! 1. There are two sources of reachability at play -- the data structure, and
//! the snapshots in threads accessing it. Before we delete a node, we need to know
//! that it cannot be reached in either of these ways.
//!
//! 2. Once a node has been unliked from the data structure, no *new* snapshots
//! reaching it will be created.
//!
//! Using the epoch scheme is fairly straightforward, and does not require
//! understanding any of the implementation details:
//!
//! - When operating on a shared data structure, a thread must "pin the current
//! epoch", which is done by calling `pin()`. This function returns a `Guard`
//! which unpins the epoch when destroyed.
//!
//! - When the thread subsequently reads from a lock-free data structure, the
//! pointers it extracts act like references with lifetime tied to the
//! `Guard`. This allows threads to safely read from snapshotted data, being
//! guaranteed that the data will remain allocated until they exit the epoch.
//!
//! To put the `Guard` to use, Crossbeam provides a set of three pointer types meant to work together:
//!
//! - `Owned<T>`, akin to `Box<T>`, which points to uniquely-owned data that has
//!   not yet been published in a concurrent data structure.
//!
//! - `Shared<'a, T>`, akin to `&'a T`, which points to shared data that may or may
//!   not be reachable from a data structure, but it guaranteed not to be freed
//!   during lifetime `'a`.
//!
//! - `Atomic<T>`, akin to `std::sync::atomic::AtomicPtr`, which provides atomic
//!   updates to a pointer using the `Owned` and `Shared` types, and connects them
//!   to a `Guard`.
//!
//! Each of these types provides further documentation on usage.
//!
//! # Example
//!
//! ```
//! use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
//! use std::ptr;
//!
//! use crossbeam::mem::epoch::{self, Atomic, Owned};
//!
//! struct TreiberStack<T> {
//!     head: Atomic<Node<T>>,
//! }
//!
//! struct Node<T> {
//!     data: T,
//!     next: Atomic<Node<T>>,
//! }
//!
//! impl<T> TreiberStack<T> {
//!     fn new() -> TreiberStack<T> {
//!         TreiberStack {
//!             head: Atomic::null()
//!         }
//!     }
//!
//!     fn push(&self, t: T) {
//!         // allocate the node via Owned
//!         let mut n = Owned::new(Node {
//!             data: t,
//!             next: Atomic::null(),
//!         });
//!
//!         // become active
//!         let guard = epoch::pin();
//!
//!         loop {
//!             // snapshot current head
//!             let head = self.head.load(Relaxed, &guard);
//!
//!             // update `next` pointer with snapshot
//!             n.next.store_shared(head, Relaxed);
//!
//!             // if snapshot is still good, link in the new node
//!             match self.head.cas_and_ref(head, n, Release, &guard) {
//!                 Ok(_) => return,
//!                 Err(owned) => n = owned,
//!             }
//!         }
//!     }
//!
//!     fn pop(&self) -> Option<T> {
//!         // become active
//!         let guard = epoch::pin();
//!
//!         loop {
//!             // take a snapshot
//!             match self.head.load(Acquire, &guard) {
//!                 // the stack is non-empty
//!                 Some(head) => {
//!                     // read through the snapshot, *safely*!
//!                     let next = head.next.load(Relaxed, &guard);
//!
//!                     // if snapshot is still good, update from `head` to `next`
//!                     if self.head.cas_shared(Some(head), next, Release) {
//!                         unsafe {
//!                             // mark the node as unlinked
//!                             guard.unlinked(head);
//!
//!                             // extract out the data from the now-unlinked node
//!                             return Some(ptr::read(&(*head).data))
//!                         }
//!                     }
//!                 }
//!
//!                 // we observed the stack empty
//!                 None => return None
//!             }
//!         }
//!     }
//! }
//! ```

// FIXME: document implementation details

use std::marker::PhantomData;
use std::marker;
use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::atomic::{self, Ordering};

mod participant;
mod participants;
mod global;
mod local;
mod garbage;

/// Like `Box<T>`: an owned, heap-allocated data value of type `T`.
pub struct Owned<T> {
    data: Box<T>,
}

impl<T> Owned<T> {
    /// Move `t` to a new heap allocation.
    pub fn new(t: T) -> Owned<T> {
        Owned { data: Box::new(t) }
    }

    fn as_raw(&self) -> *mut T {
        self.deref() as *const _ as *mut _
    }

    /// Move data out of the owned box, deallocating the box.
    pub fn into_inner(self) -> T {
        *self.data
    }
}

impl<T> Deref for Owned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

#[derive(PartialEq, Eq)]
/// Like `&'a T`: a shared reference valid for lifetime `'a`.
pub struct Shared<'a, T: 'a> {
    data: &'a T,
}

impl<'a, T> Copy for Shared<'a, T> {}
impl<'a, T> Clone for Shared<'a, T> {
    fn clone(&self) -> Shared<'a, T> {
        Shared { data: self.data }
    }
}

impl<'a, T> Deref for Shared<'a, T> {
    type Target = &'a T;
    fn deref(&self) -> &&'a T {
        &self.data
    }
}

impl<'a, T> Shared<'a, T> {
    unsafe fn from_raw(raw: *mut T) -> Option<Shared<'a, T>> {
        if raw == ptr::null_mut() { None }
        else {
            Some(Shared {
                data: mem::transmute::<*mut T, &T>(raw)
            })
        }
    }

    unsafe fn from_ref(r: &T) -> Shared<'a, T> {
        Shared { data: mem::transmute(r) }
    }

    unsafe fn from_owned(owned: Owned<T>) -> Shared<'a, T> {
        let ret = Shared::from_ref(owned.deref());
        mem::forget(owned);
        ret
    }

    pub fn as_raw(&self) -> *mut T {
        self.data as *const _ as *mut _
    }
}

/// Like `std::sync::atomic::AtomicPtr`.
///
/// Provides atomic access to a (nullable) pointer of type `T`, interfacing with
/// the `Owned` and `Shared` types.
pub struct Atomic<T> {
    ptr: atomic::AtomicPtr<T>,
    _marker: PhantomData<*const ()>,
}

unsafe impl<T: Sync> Send for Atomic<T> {}
unsafe impl<T: Sync> Sync for Atomic<T> {}

fn opt_shared_into_raw<T>(val: Option<Shared<T>>) -> *mut T {
    val.map(|p| p.as_raw()).unwrap_or(ptr::null_mut())
}

fn opt_owned_as_raw<T>(val: &Option<Owned<T>>) -> *mut T {
    val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut())
}

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
            _marker: PhantomData
        }
    }

    #[cfg(not(feature = "nightly"))]
    pub fn null() -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(0 as *mut _),
            _marker: PhantomData
        }
    }

    /// Create a new atomic pointer
    pub fn new(data: T) -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(Box::into_raw(Box::new(data))),
            _marker: PhantomData
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
    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> Option<Shared<'a, T>> {
        unsafe { Shared::from_raw(self.ptr.load(ord)) }
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
    pub fn store_and_ref<'a>(&self, val: Owned<T>, ord: Ordering, _: &'a Guard)
                             -> Shared<'a, T>
    {
        unsafe {
            let shared = Shared::from_owned(val);
            self.store_shared(Some(shared), ord);
            shared
        }
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

    /// Do a compare-and-set from a `Shared` to an `Owned` pointer with the
    /// given memory ordering.
    ///
    /// As with `store`, this operation does not require a guard; it produces no new
    /// lifetime information. The `Result` indicates whether the CAS succeeded; if
    /// not, ownership of the `new` pointer is returned to the caller.
    pub fn cas(&self, old: Option<Shared<T>>, new: Option<Owned<T>>, ord: Ordering)
               -> Result<(), Option<Owned<T>>>
    {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old),
                                     opt_owned_as_raw(&new),
                                     ord) == opt_shared_into_raw(old)
        {
            mem::forget(new);
            Ok(())
        } else {
            Err(new)
        }
    }

    /// Do a compare-and-set from a `Shared` to an `Owned` pointer with the
    /// given memory ordering, immediatley acquiring a new `Shared` reference to
    /// the previously-owned pointer if successful.
    ///
    /// This operation is analogous to `store_and_ref`.
    pub fn cas_and_ref<'a>(&self, old: Option<Shared<T>>, new: Owned<T>,
                           ord: Ordering, _: &'a Guard)
                           -> Result<Shared<'a, T>, Owned<T>>
    {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old), new.as_raw(), ord)
            == opt_shared_into_raw(old)
        {
            Ok(unsafe { Shared::from_owned(new) })
        } else {
            Err(new)
        }
    }

    /// Do a compare-and-set from a `Shared` to another `Shared` pointer with
    /// the given memory ordering.
    ///
    /// The boolean return value is `true` when the CAS is successful.
    pub fn cas_shared(&self, old: Option<Shared<T>>, new: Option<Shared<T>>, ord: Ordering)
                      -> bool
    {
        self.ptr.compare_and_swap(opt_shared_into_raw(old),
                                  opt_shared_into_raw(new),
                                  ord) == opt_shared_into_raw(old)
    }

    /// Do an atomic swap with an `Owned` pointer with the given memory ordering.
    pub fn swap<'a>(&self, new: Option<Owned<T>>, ord: Ordering, _: &'a Guard)
                    -> Option<Shared<'a, T>> {
        unsafe { Shared::from_raw(self.ptr.swap(opt_owned_into_raw(new), ord)) }
    }

    /// Do an atomic swap with a `Shared` pointer with the given memory ordering.
    pub fn swap_shared<'a>(&self, new: Option<Shared<T>>, ord: Ordering, _: &'a Guard)
                           -> Option<Shared<'a, T>> {
        unsafe { Shared::from_raw(self.ptr.swap(opt_shared_into_raw(new), ord)) }
    }
}

/// An RAII-style guard for pinning the current epoch.
///
/// A guard must be acquired before most operations on an `Atomic` pointer. On
/// destruction, it unpins the epoch.
#[must_use]
pub struct Guard {
    _marker: marker::PhantomData<*mut ()>, // !Send and !Sync
}

/// Pin the current epoch.
///
/// Threads generally pin before interacting with a lock-free data
/// structure. Pinning requires a full memory barrier, so is somewhat
/// expensive. It is rentrant -- you can safely acquire nested guards, and only
/// the first guard requires a barrier. Thus, in cases where you expect to
/// perform several lock-free operations in quick succession, you may consider
/// pinning around the entire set of operations.
pub fn pin() -> Guard {
    local::with_participant(|p| {
        p.enter();

        let g = Guard {
            _marker: marker::PhantomData,
        };

        if p.should_gc() {
            p.try_collect(&g);
        }

        g
    })
}

impl Guard {
    /// Assert that the value is no longer reachable from a lock-free data
    /// structure and should be collected when sufficient epochs have passed.
    pub unsafe fn unlinked<T>(&self, val: Shared<T>) {
        local::with_participant(|p| p.reclaim(val.as_raw()))
    }

    /// Move the thread-local garbage into the global set of garbage.
    pub fn migrate_garbage(&self) {
        local::with_participant(|p| p.migrate_garbage())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        local::with_participant(|p| p.exit());
    }
}

#[cfg(test)]
mod test {
    use std::sync::atomic::Ordering;
    use super::*;
    use mem::epoch;

    #[test]
    fn test_no_drop() {
        static mut DROPS: i32 = 0;
        struct Test;
        impl Drop for Test {
            fn drop(&mut self) {
                unsafe {
                    DROPS += 1;
                }
            }
        }
        let g = pin();

        let x = Atomic::null();
        x.store(Some(Owned::new(Test)), Ordering::Relaxed);
        x.store_and_ref(Owned::new(Test), Ordering::Relaxed, &g);
        let y = x.load(Ordering::Relaxed, &g);
        let z = x.cas_and_ref(y, Owned::new(Test), Ordering::Relaxed, &g).ok();
        let _ = x.cas(z, Some(Owned::new(Test)), Ordering::Relaxed);
        x.swap(Some(Owned::new(Test)), Ordering::Relaxed, &g);

        unsafe {
            assert_eq!(DROPS, 0);
        }
    }

    #[test]
    fn test_new() {
        let guard = epoch::pin();
        let my_atomic = Atomic::new(42);

        assert_eq!(**my_atomic.load(Ordering::Relaxed, &guard).unwrap(), 42);
    }
}
