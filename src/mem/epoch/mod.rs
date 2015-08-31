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

use std::cell::UnsafeCell;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{self, Relaxed, Acquire, Release, SeqCst};
use std::ops::{Deref, DerefMut};
use std::marker::PhantomData;

use mem::CachePadded;

mod garbage;

/// Global, threadsafe list of threads participating in epoch management.
struct Participants {
    head: Atomic<ParticipantNode>
}

struct ParticipantNode(CachePadded<Participant>);

impl ParticipantNode {
    fn new(p: Participant) -> ParticipantNode {
        ParticipantNode(CachePadded::new(p))
    }
}

impl Deref for ParticipantNode {
    type Target = Participant;
    fn deref(&self) -> &Participant {
        &self.0
    }
}

impl DerefMut for ParticipantNode {
    fn deref_mut(&mut self) -> &mut Participant {
        &mut self.0
    }
}

unsafe impl Sync for Participant {}

/// Thread-local data for epoch participation.
struct Participant {
    /// The local epoch.
    epoch: AtomicUsize,

    /// Number of pending uses of `epoch::pin()`; keeping a count allows for
    /// reentrant use of epoch management.
    in_critical: AtomicUsize,

    /// Is the thread still active? Becomes `false` when the thread exits. This
    /// is ultimately used to free `Participant` records.
    active: AtomicBool,

    /// Thread-local garbage tracking
    garbage: UnsafeCell<garbage::Local>,

    /// The participant list is coded intrusively; here's the `next` pointer.
    next: Atomic<ParticipantNode>,
}

impl Participants {
    const fn new() -> Participants {
        Participants { head: Atomic::null() }
    }

    /// Enroll a new thread in epoch management by adding a new `Particpant`
    /// record to the global list.
    fn enroll(&self) -> *const Participant {
        let mut participant = Owned::new(ParticipantNode::new(
            Participant {
                epoch: AtomicUsize::new(0),
                in_critical: AtomicUsize::new(0),
                active: AtomicBool::new(true),
                garbage: UnsafeCell::new(garbage::Local::new()),
                next: Atomic::null(),
            }
        ));

        // we ultimately use epoch tracking to free Participant nodes, but we
        // can't actually enter an epoch here, so fake it; we know the node
        // can't be removed until marked inactive anyway.
        let fake_guard = ();
        let g: &'static Guard = unsafe { mem::transmute(&fake_guard) };
        loop {
            let head = self.head.load(Relaxed, g);
            participant.next.store_shared(head, Relaxed);
            match self.head.cas_and_ref(head, participant, Release, g) {
                Ok(shared) => {
                    let shared: &Participant = &shared;
                    return shared;
                }
                Err(owned) => {
                    participant = owned;
                }
            }
        }
    }

    fn iter<'a>(&'a self, g: &'a Guard) -> Iter<'a> {
        Iter {
            guard: g,
            next: &self.head,
            needs_acq: true,
        }
    }
}

struct Iter<'a> {
    // pin to an epoch so that we can free inactive nodes
    guard: &'a Guard,
    next: &'a Atomic<ParticipantNode>,

    // an Acquire read is needed only for the first read, due to release
    // sequences
    needs_acq: bool,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        let mut cur = if self.needs_acq {
            self.needs_acq = false;
            self.next.load(Acquire, self.guard)
        } else {
            self.next.load(Relaxed, self.guard)
        };

        while let Some(n) = cur {
            // attempt to clean up inactive nodes
            if !n.active.load(Relaxed) {
                cur = n.next.load(Relaxed, self.guard);
                unsafe {
                    if self.next.cas_shared(Some(n), cur, Relaxed) {
                        self.guard.unlinked(n)
                    }
                }
                self.next = &n.next;
            } else {
                self.next = &n.next;
                return Some(&n)
            }
        }

        None
    }
}

/// Global epoch state
struct EpochState {
    /// Current global epoch
    epoch: CachePadded<AtomicUsize>,

    /// Global garbage bags
    garbage: [CachePadded<garbage::ConcBag>; 3],

    /// Participant list
    participants: Participants,
}

unsafe impl Send for EpochState {}
unsafe impl Sync for EpochState {}

impl EpochState {
    const fn new() -> EpochState {
        EpochState {
            epoch: CachePadded::zeroed(),
            garbage: [CachePadded::zeroed(),
                      CachePadded::zeroed(),
                      CachePadded::zeroed()],
            participants: Participants::new(),
        }
    }
}

static EPOCH: EpochState = EpochState::new();

impl Participant {
    fn enter(&self) {
        let new_count = self.in_critical.load(Relaxed) + 1;
        self.in_critical.store(new_count, Relaxed);
        if new_count > 1 { return }

        atomic::fence(SeqCst);

        let global_epoch = EPOCH.epoch.load(Relaxed);
        if global_epoch != self.epoch.load(Relaxed) {
            self.epoch.store(global_epoch, Relaxed);
            unsafe { (*self.garbage.get()).collect(); }
        }
    }

    fn exit(&self) {
        let new_count = self.in_critical.load(Relaxed) - 1;
        self.in_critical.store(
            new_count,
            if new_count > 1 { Relaxed } else { Release });
    }

    unsafe fn reclaim<T>(&self, data: *mut T) {
        (*self.garbage.get()).reclaim(data);
    }

    fn try_collect(&self) -> bool {
        let cur_epoch = EPOCH.epoch.load(SeqCst);

        let fake_guard = ();
        let g: &'static Guard = unsafe { mem::transmute(&fake_guard) };

        for p in EPOCH.participants.iter(g) {
            if p.in_critical.load(Relaxed) > 0 && p.epoch.load(Relaxed) != cur_epoch {
                return false
            }
        }

        let new_epoch = cur_epoch.wrapping_add(1);
        atomic::fence(Acquire);
        if EPOCH.epoch.compare_and_swap(cur_epoch, new_epoch, SeqCst) != cur_epoch {
            return false
        }

        self.epoch.store(new_epoch, Relaxed);

        unsafe {
            EPOCH.garbage[new_epoch.wrapping_add(1) % 3].collect();
        }

        true
    }

    fn migrate_garbage(&self) {
        let cur_epoch = self.epoch.load(Relaxed);
        let local = unsafe { mem::replace(&mut *self.garbage.get(), garbage::Local::new()) };
        EPOCH.garbage[cur_epoch.wrapping_sub(1) % 3].insert(local.old);
        EPOCH.garbage[cur_epoch % 3].insert(local.cur);
        EPOCH.garbage[EPOCH.epoch.load(Relaxed) % 3].insert(local.new);
    }
}

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

    fn as_raw(&self) -> *mut T {
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

impl<T> Atomic<T> {
    /// Create a new, null atomic pointer.
    pub const fn null() -> Atomic<T> {
        Atomic {
            ptr: atomic::AtomicPtr::new(0 as *mut _),
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
    /// Panics if `order` is `Acquire` or `AcqRel`.
    pub fn store(&self, val: Option<Owned<T>>, ord: Ordering) {
        self.ptr.store(opt_owned_as_raw(&val), ord)
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
    /// Panics if `order` is `Acquire` or `AcqRel`.
    pub fn store_and_ref<'a>(&self, val: Owned<T>, ord: Ordering, _: &'a Guard) -> Shared<'a, T> {
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
    /// Panics if `order` is `Acquire` or `AcqRel`.
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
        unsafe { Shared::from_raw(self.ptr.swap(opt_owned_as_raw(&new), ord)) }
    }

    /// Do an atomic swap with a `Shared` pointer with the given memory ordering.
    pub fn swap_shared<'a>(&self, new: Option<Shared<T>>, ord: Ordering, _: &'a Guard)
                                  -> Option<Shared<'a, T>> {
        unsafe { Shared::from_raw(self.ptr.swap(opt_shared_into_raw(new), ord)) }
    }
}

struct LocalEpoch {
    participant: *const Participant,
}

impl LocalEpoch {
    fn new() -> LocalEpoch {
        LocalEpoch { participant: EPOCH.participants.enroll() }
    }

    fn get(&self) -> &Participant {
        unsafe { &*self.participant }
    }
}

// FIXME: avoid leaking when all threads have exited
impl Drop for LocalEpoch {
    fn drop(&mut self) {
        let p = self.get();
        p.enter();
        p.migrate_garbage();
        p.exit();
        p.active.store(false, Relaxed);
    }
}

thread_local!(static LOCAL_EPOCH: LocalEpoch = LocalEpoch::new() );

/// An RAII-style guard for pinning the current epoch.
///
/// A guard must be acquired before most operations on an `Atomic` pointer. On
/// destruction, it unpins the epoch.
#[must_use]
pub struct Guard {
    _dummy: ()
}

static GC_THRESH: usize = 32;

fn with_participant<F, T>(f: F) -> T where F: FnOnce(&Participant) -> T {
    LOCAL_EPOCH.with(|e| f(e.get()))
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
    with_participant(|p| {
        p.enter();
        if unsafe { (*p.garbage.get()).size() } > GC_THRESH {
            p.try_collect();
        }
    });
    Guard {
        _dummy: ()
    }
}

impl Guard {
    /// Assert that the value is no longer reachable from a lock-free data
    /// structure and should be collected when sufficient epochs have passed.
    pub unsafe fn unlinked<T>(&self, val: Shared<T>) {
        with_participant(|p| p.reclaim(val.as_raw()))
    }

    /// Move the thread-local garbage into the global set of garbage.
    pub fn migrate_garbage(&self) {
        with_participant(|p| p.migrate_garbage())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        with_participant(|p| p.exit());
    }
}

impl !Send for Guard {}
impl !Sync for Guard {}

#[cfg(test)]
mod test {
    use super::{Participants, EPOCH};
    use super::*;

    #[test]
    fn smoke_enroll() {
        Participants::new().enroll();
    }

    #[test]
    fn smoke_enroll_EPOCH() {
        EPOCH.participants.enroll();
    }

    #[test]
    fn smoke_guard() {
        let g = pin();
    }
}
