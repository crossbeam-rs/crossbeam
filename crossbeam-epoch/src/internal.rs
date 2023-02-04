//! The global data and participant for garbage collection.
//!
//! # Registration
//!
//! In order to track all participants in one place, we need some form of participant
//! registration. When a participant is created, it is registered to a global lock-free
//! singly-linked list of registries; and when a participant is leaving, it is unregistered from the
//! list.
//!
//! # Pinning
//!
//! Every participant contains an integer that tells whether the participant is pinned and if so,
//! what was the global epoch at the time it was pinned. Participants also hold a pin counter that
//! aids in periodic global epoch advancement.
//!
//! When a participant is pinned, a `Guard` is returned as a witness that the participant is pinned.
//! Guards are necessary for performing atomic operations, and for freeing/dropping locations.
//!
//! # Thread-local bag
//!
//! Objects that get unlinked from concurrent data structures must be stashed away until the global
//! epoch sufficiently advances so that they become safe for destruction. Pointers to such objects
//! are pushed into a thread-local bag, and when it becomes full, the bag is marked with the current
//! global epoch and pushed into the global queue of bags. We store objects in thread-local storages
//! for amortizing the synchronization cost of pushing the garbages to a global queue.
//!
//! # Global queue
//!
//! Whenever a bag is pushed into a queue, the objects in some bags in the queue are collected and
//! destroyed along the way. This design reduces contention on data structures. The global queue
//! cannot be explicitly accessed: the only way to interact with it is by calling functions
//! `defer()` that adds an object to the thread-local bag, or `collect()` that manually triggers
//! garbage collection.
//!
//! Ideally each instance of concurrent data structure may have its own queue that gets fully
//! destroyed as soon as the data structure gets dropped.

use crate::primitive::cell::UnsafeCell;
use crate::sync::striped_refcount::StripedRefcount;
use core::cell::Cell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;

use alloc::rc::Rc;
use crossbeam_utils::CachePadded;

use crate::collector::{Collector, LocalHandle};
use crate::deferred::Deferred;
use crate::primitive::sync::atomic::{AtomicPtr, AtomicUsize};

enum PushResult {
    Done,
    ShouldRealloc,
}

struct Segment {
    len: CachePadded<AtomicUsize>,
    deferreds: Box<[MaybeUninit<UnsafeCell<Deferred>>]>,
}

// For some reason using Owned<> for this gives stacked borrows errors
fn uninit_boxed_slice<T>(len: usize) -> Box<[MaybeUninit<T>]> {
    unsafe {
        let ptr = if len == 0 {
            core::mem::align_of::<T>() as *mut MaybeUninit<T>
        } else {
            alloc::alloc::alloc(alloc::alloc::Layout::array::<T>(len).unwrap()) as _
        };
        Box::from_raw(core::slice::from_raw_parts_mut(ptr, len))
    }
}

impl Segment {
    #[cfg(crossbeam_loom)]
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        let mut deferreds = uninit_boxed_slice(capacity);
        // Work around lack of UnsafeCell::raw_get (1.56)
        deferreds.iter_mut().for_each(|uninit| {
            uninit.write(UnsafeCell::new(Deferred::NO_OP));
        });
        Self {
            len: CachePadded::new(AtomicUsize::new(0)),
            deferreds,
        }
    }
    #[cfg(not(crossbeam_loom))]
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            len: CachePadded::new(AtomicUsize::new(0)),
            deferreds: uninit_boxed_slice(capacity),
        }
    }
    fn capacity(&self) -> usize {
        self.deferreds.len()
    }
    fn call(&self) {
        let end = self.capacity().min(self.len.load(Ordering::Relaxed));
        for deferred in &self.deferreds[..end] {
            unsafe { deferred.assume_init_read().with(|ptr| ptr.read().call()) }
        }
        self.len.store(0, Ordering::Relaxed);
    }
    fn try_push(&self, deferred: &mut Vec<Deferred>) -> PushResult {
        let slot = self.len.fetch_add(deferred.len(), Ordering::Relaxed);
        if slot >= self.capacity() {
            return PushResult::Done;
        }
        let end = slot + deferred.len();
        let not_pushed = end.saturating_sub(self.capacity());
        unsafe {
            for (slot, deferred) in self.deferreds[slot..]
                .iter()
                .zip(deferred.drain(not_pushed..))
            {
                // Work around lack of UnsafeCell::raw_get (1.56)
                #[cfg(crossbeam_loom)]
                slot.assume_init_ref().with_mut(|slot| slot.write(deferred));
                #[cfg(not(crossbeam_loom))]
                core::ptr::write(slot.as_ptr() as *mut _, deferred);
            }
        };
        if end >= self.capacity() {
            PushResult::ShouldRealloc
        } else {
            PushResult::Done
        }
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        self.call()
    }
}

/// A bag of deferred functions.
pub(crate) struct Bag {
    /// Stashed objects.
    current: AtomicPtr<Segment>,
}

impl Bag {
    /// This must be synchronized with emptying the bag
    pub(crate) unsafe fn try_push(&self, deferred: &mut Vec<Deferred>) {
        let segment = self.current.load(Ordering::Acquire);
        if let PushResult::ShouldRealloc = (*segment).try_push(deferred) {
            println!("{}", (*segment).capacity() * 2);
            let next = Box::into_raw(Box::new(Segment::new((*segment).capacity() * 2)));
            self.current.store(next, Ordering::Release);
            deferred.push(Deferred::new(move || drop(Box::from_raw(segment))));
            // We're potentially holding a lot of garbage now, make an attempt to get rid of it sooner
            self.try_push(deferred)
        }
    }

    pub(crate) unsafe fn call(&self) {
        (*self.current.load(Ordering::Relaxed)).call()
    }
}

impl Default for Bag {
    fn default() -> Self {
        Bag {
            current: AtomicPtr::new(Box::into_raw(Box::new(Segment::new(16)))),
        }
    }
}

impl Drop for Bag {
    fn drop(&mut self) {
        unsafe {
            // Ordering is taken care of because `Global` is behind an `Arc`
            self.call();
            drop(Box::from_raw(self.current.load(Ordering::Relaxed)));
        }
    }
}

impl fmt::Debug for Bag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bag").finish_non_exhaustive()
    }
}

/// The global data for a garbage collector.
#[derive(Default)]
pub(crate) struct Global {
    /// The global epoch.
    pub(crate) epoch: CachePadded<AtomicUsize>,
    /// The number of threads pinned in each epoch, plus one for epochs that
    /// aren't allowed to be collected right now.
    pins: [StripedRefcount; 4],
    garbage: [Bag; 4],
}

impl Global {
    /// Creates a new global data for garbage collection.
    #[inline]
    pub(crate) fn new() -> Self {
        Default::default()
    }

    /// Attempt to collect global garbage and advance the epoch
    ///
    /// Note: This may itself produce garbage and in turn allocate new bags.
    ///
    /// `pin()` rarely calls `collect()`, so we want the compiler to place that call on a cold
    /// path. In other words, we want the compiler to optimize branching for the case when
    /// `collect()` is not called.
    pub(crate) fn collect(&self, local: &Local) {
        let next = (local.epoch() + 1) % 4;
        let previous2 = (local.epoch() + 2) % 4;
        let previous = (local.epoch() + 3) % 4;
        if
        // Sync with uses of garbage in previous epoch
        self.pins[previous].load(Ordering::Acquire) == 0
            // Lock out other calls, and sync with next epoch
                && self
                    .epoch
                    .compare_exchange(local.epoch(), next, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
        {
            unsafe { self.garbage[previous2].call() }
        }
    }
}

/// Participant for garbage collection.
pub(crate) struct Local {
    id: usize,

    /// The local epoch.
    epoch: Cell<usize>,

    /// A reference to the global data.
    ///
    /// When all guards and handles get dropped, this reference is destroyed.
    collector: Collector,

    /// The number of guards keeping this participant pinned.
    guard_count: Cell<usize>,

    buffer: UnsafeCell<Vec<Deferred>>,
}

// Make sure `Local` is less than or equal to 2048 bytes.
// https://github.com/crossbeam-rs/crossbeam/issues/551
#[cfg(not(any(crossbeam_sanitize, miri)))] // `crossbeam_sanitize` and `miri` reduce the size of `Local`
#[test]
fn local_size() {
    // TODO: https://github.com/crossbeam-rs/crossbeam/issues/869
    // assert!(
    //     core::mem::size_of::<Local>() <= 2048,
    //     "An allocation of `Local` should be <= 2048 bytes."
    // );
}

#[cfg(not(crossbeam_loom))]
static LOCAL_ID: AtomicUsize = AtomicUsize::new(0);

impl Local {
    /// Number of defers after which a participant will execute some deferred functions from the
    /// global queue.
    const DEFERS_BETWEEN_COLLECT: usize = 8;

    /// Registers a new `Local` in the provided `Global`.
    pub(crate) fn register(collector: &Collector) -> LocalHandle {
        let local = Rc::new(Local {
            #[cfg(not(crossbeam_loom))]
            id: LOCAL_ID.fetch_add(1, Ordering::Relaxed),
            #[cfg(crossbeam_loom)]
            id: 0,
            epoch: Cell::new(0),
            collector: collector.clone(),
            guard_count: Cell::new(0),
            buffer: UnsafeCell::new(vec![]),
        });
        LocalHandle { local }
    }

    /// Returns a reference to the `Global` in which this `Local` resides.
    #[inline]
    pub(crate) fn global(&self) -> &Global {
        &self.collector().global
    }

    /// Returns a reference to the `Collector` in which this `Local` resides.
    #[inline]
    pub(crate) fn collector(&self) -> &Collector {
        &self.collector
    }

    /// Returns `true` if the current participant is pinned.
    #[inline]
    pub(crate) fn is_pinned(&self) -> bool {
        self.guard_count.get() > 0
    }

    #[inline]
    pub(crate) fn epoch(&self) -> usize {
        self.epoch.get()
    }

    /// Adds `deferred` to the thread-local bag.
    ///
    /// # Safety
    ///
    /// It should be safe for another thread to execute the given function.
    pub(crate) unsafe fn defer(&self, deferred: Deferred) {
        // Safety: We are !Send and !Sync and not handing out &mut's
        let buffered = self.buffer.with_mut(|b| {
            (*b).push(deferred);
            (*b).len()
        });

        // After every `DEFERS_BETWEEN_COLLECT` try advancing the epoch and collecting
        // some garbage.
        if buffered > Self::DEFERS_BETWEEN_COLLECT {
            self.flush();
        }
    }

    pub(crate) fn flush(&self) {
        // Safety: We are pinned to self.epoch at this point
        let bag = &self.global().garbage[self.epoch()];
        unsafe { self.buffer.with_mut(|buffer| bag.try_push(&mut *buffer)) };
        self.global().collect(self);
    }

    /// Pins the `Local`.
    #[inline]
    pub(crate) fn pin(&self) {
        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count.checked_add(1).unwrap());

        if guard_count == 0 {
            self.epoch.set(self.global().epoch.load(Ordering::Acquire));
            self.global().pins[self.epoch()].increment(self.id, Ordering::Relaxed);
        }
    }

    /// Unpins the `Local`.
    #[inline]
    pub(crate) fn unpin(&self) {
        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count - 1);

        if guard_count == 1 {
            self.global().pins[self.epoch()].decrement(self.id, Ordering::Release);
        }
    }

    /// Unpins and then pins the `Local`.
    #[inline]
    pub(crate) fn repin(&self) {
        let guard_count = self.guard_count.get();

        // Update the local epoch only if there's only one guard.
        if guard_count == 1 {
            let new_epoch = self.global().epoch.load(Ordering::Acquire);
            if self.epoch() != new_epoch {
                self.global().pins[self.epoch()].decrement(self.id, Ordering::Release);
                self.global().pins[new_epoch].increment(self.id, Ordering::Relaxed);
                self.epoch.set(new_epoch);
            }
        }
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        self.pin();
        let bag = &self.global().garbage[self.epoch()];
        self.buffer.with_mut(|buffer| unsafe {
            while !(*buffer).is_empty() {
                bag.try_push(&mut *buffer)
            }
        });
        self.unpin();
    }
}

#[cfg(all(test, not(crossbeam_loom)))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn check_defer() {
        static FLAG: AtomicUsize = AtomicUsize::new(0);
        fn set() {
            FLAG.store(42, Ordering::Relaxed);
        }

        let d = Deferred::new(set);
        assert_eq!(FLAG.load(Ordering::Relaxed), 0);
        d.call();
        assert_eq!(FLAG.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn check_bag() {
        static FLAG: AtomicUsize = AtomicUsize::new(0);
        fn incr() {
            FLAG.fetch_add(1, Ordering::Relaxed);
        }

        let bag = Bag::default();
        let mut buffer1 = (0..3).map(|_| Deferred::new(incr)).collect();
        let mut buffer2 = (0..18).map(|_| Deferred::new(incr)).collect();

        unsafe { bag.try_push(&mut buffer1) };
        assert_eq!(buffer1.len(), 0);
        assert_eq!(FLAG.load(Ordering::Relaxed), 0);
        unsafe { bag.call() };
        assert_eq!(FLAG.load(Ordering::Relaxed), 3);
        unsafe { bag.try_push(&mut buffer2) };
        assert_eq!(buffer2.len(), 0);
        assert_eq!(FLAG.load(Ordering::Relaxed), 3);
        drop(bag);
        assert_eq!(FLAG.load(Ordering::Relaxed), 21);
    }
}
