//! The global data and participant for garbage collection.
//!
//! # Pinning
//!
//! When a participant is pinned, it increments a global counter for the current epoch. When it is
//! unpinned, it decrements that counter.
//!
//! When a participant is pinned, a `Guard` is returned as a witness that the participant is pinned.
//! Guards are necessary for performing atomic operations, and for freeing/dropping locations.
//!
//! # Thread-local bag
//!
//! Objects that get unlinked from concurrent data structures must be stashed away until the global
//! epoch sufficiently advances so that they become safe for destruction. Pointers to such objects
//! are pushed into a thread-local array, and when it becomes larger than a certain size, the items
//! are moved to the global garbage pile for the current epock. We store objects in thread-local
//! storage to reduce contention on the global data structure. Whenever objects are put on a global
//! pile, the current thread also tries to advance the epoch.
//!
//! # Ordering
//!
//! There may be threads pinned in up to two epochs at once. This is required for wait-freedom.
//! A thread can advance the epoch once it sees that no threads are pinned in the epoch before it.
//! This ensures that all pins in epoch n happen-before all pins in epoch n+2, and the thread that
//! advances from n+1 to n+2 additionally happens-after all pins in n+1. The first condition means
//! that if a thread pinned in n unlinks a pointer from the concurrent data structure, only pins in
//! n+1 and earler could have found it, and thus the thread that advanced to n+2 may safely delete
//! it.
//!
//! # Global piles
//!
//! Each epoch has its own global pile of garbage. The piles may be concurrently inserted into,
//! but clearing them is not thread safe. Instead, we use four epochs. The pile filled in n is
//! cleared during n+2, and everything in n+2 happens-before n+4, so n+4 can re-use n's pile.
//!
//! Ideally each instance of concurrent data structure may have its own queue that gets fully
//! destroyed as soon as the data structure gets dropped.

use crate::primitive::cell::UnsafeCell;
use crate::sync::rwlock::RwLock;
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

/// An array of [Deferred]
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

    /// # Safety:
    /// This must not be called concurrently with [call()].
    unsafe fn try_push(&self, deferred: &mut Vec<Deferred>) -> PushResult {
        let slot = self.len.fetch_add(deferred.len(), Ordering::Relaxed);
        if slot >= self.capacity() {
            return PushResult::Done;
        }
        let end = slot + deferred.len();
        let not_pushed = end.saturating_sub(self.capacity());
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
        if end >= self.capacity() {
            PushResult::ShouldRealloc
        } else {
            PushResult::Done
        }
    }

    /// # Safety:
    /// This must not be called concurrently with [try_push()] or [call()].
    unsafe fn call(&self) {
        let end = self.capacity().min(self.len.load(Ordering::Relaxed));
        for deferred in &self.deferreds[..end] {
            deferred.assume_init_read().with(|ptr| ptr.read().call())
        }
        self.len.store(0, Ordering::Relaxed);
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        return;
        unsafe { self.call() }
    }
}

/// A stack of garbage.
struct Pile {
    /// Stashed objects.
    current: AtomicPtr<Segment>,
}

impl Pile {
    /// # Safety:
    /// This must not be called concurrently with [call()].
    unsafe fn try_push(&self, deferred: &mut Vec<Deferred>) {
        let segment = self.current.load(Ordering::Acquire);
        if let PushResult::ShouldRealloc = (*segment).try_push(deferred) {
            let next = Box::into_raw(Box::new(Segment::new((*segment).capacity() * 2)));
            // Synchronize initialization of the Segment
            self.current.store(next, Ordering::Release);
            deferred.push(Deferred::new(move || drop(Box::from_raw(segment))));
            // We're potentially holding a lot of garbage now, make an attempt to get rid of it sooner
            self.try_push(deferred)
        }
    }

    /// # Safety:
    /// This must not be called concurrently with [try_push()] or [call()].
    unsafe fn call(&self) {
        (*self.current.load(Ordering::Relaxed)).call()
    }
}

impl Default for Pile {
    fn default() -> Self {
        Pile {
            current: AtomicPtr::new(Box::into_raw(Box::new(Segment::new(64)))),
        }
    }
}

impl Drop for Pile {
    fn drop(&mut self) {
        unsafe {
            // Ordering is taken care of because `Global` is behind an `Arc`
            drop(Box::from_raw(self.current.load(Ordering::Relaxed)));
        }
    }
}

impl fmt::Debug for Pile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pile").finish_non_exhaustive()
    }
}

/// The global data for a garbage collector.
#[derive(Debug)]
pub(crate) struct Global {
    /// The global epoch.
    pub(crate) epoch: CachePadded<AtomicUsize>,
    /// The number of threads pinned in each epoch, plus one for epochs that
    /// aren't allowed to be collected right now.
    pins: [RwLock; 3],
    garbage: [Pile; 3],
}

/// Participant for garbage collection.
pub(crate) struct Local {
    /// A reference to the global data.
    ///
    /// When all guards and handles get dropped, this reference is destroyed.
    collector: Collector,

    /// The local epoch.
    epoch: Cell<usize>,

    /// The number of guards keeping this participant pinned.
    guard_count: Cell<usize>,

    /// Locally stored Deferred functions. Pushed in bulk to reduce contention.
    buffer: UnsafeCell<Vec<Deferred>>,

    /// A different number for each Local. Used as a hint for the [RwLock].
    id: usize,
}

// Make sure `Local` is less than or equal to 2048 bytes.
#[test]
fn local_size() {
    assert!(
        core::mem::size_of::<Local>() <= 2048,
        "An allocation of `Local` should be <= 2048 bytes."
    );
}

#[cfg(not(crossbeam_loom))]
static LOCAL_ID: AtomicUsize = AtomicUsize::new(0);
#[cfg(crossbeam_loom)]
loom::lazy_static! {
    // AtomicUsize::new is not const in Loom.
    static ref LOCAL_ID: AtomicUsize = AtomicUsize::new(0);
}

impl Local {
    /// Number of defers after which a participant will execute some deferred functions from the
    /// global queue.
    const DEFERS_BETWEEN_COLLECT: usize = 16;

    /// Create a new [Local]
    pub(crate) fn new(collector: &Collector) -> LocalHandle {
        let local = Rc::new(Local {
            id: LOCAL_ID.fetch_add(1, Ordering::Relaxed),
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

    /// Returns the most recent epoch the participant was pinned in.
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
}

impl Local {
    /// Pins the `Local`.
    #[inline]
    pub(crate) fn pin(&self) {
        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count.checked_add(1).unwrap());

        if guard_count == 0 {
            let mut epoch = self.global().epoch.load(Ordering::Relaxed);
            while !self.global().pins[epoch].try_rlock(self.id) {
                epoch = (epoch + 1) % 3;
            }
            println!("{}: rlock({})", self.id, epoch);
            self.epoch.set(epoch);
        }
    }

    /// Unpins the `Local`.
    #[inline]
    pub(crate) fn unpin(&self) {
        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count - 1);

        if guard_count == 1 {
            println!("{}: runlock({})", self.id, self.epoch());
            self.global().pins[self.epoch()].runlock(self.id);
        }
    }

    /// Unpins and then pins the `Local`.
    #[inline]
    pub(crate) fn repin(&self) {
        let guard_count = self.guard_count.get();

        // Update the local epoch only if there's only one guard.
        if guard_count == 1 {
            let mut new_epoch = self.global().epoch.load(Ordering::Relaxed);
            if self.epoch() != new_epoch {
                println!("{}: runlock({})", self.id, self.epoch());
                self.global().pins[self.epoch()].runlock(self.id);
                while !self.global().pins[new_epoch].try_rlock(self.id) {
                    new_epoch = (new_epoch + 1) % 3;
                }
                println!("{}: rlock({})", self.id, new_epoch);
                self.epoch.set(new_epoch);
            }
        }
    }

    /// `defer()` rarely calls `flush()`, so we want the compiler to place that call on a
    /// cold path. In other words, we want the compiler to optimize branching for the case
    /// when `flush()` is not called.
    #[cold]
    pub(crate) fn flush(&self) {
        // Safety: We are pinned to self.epoch at this point
        let bag = &self.global().garbage[self.epoch()];
        unsafe { self.buffer.with_mut(|buffer| bag.try_push(&mut *buffer)) };
        self.global().collect(self);
    }
}

impl Default for Global {
    fn default() -> Self {
        let result = Global {
            epoch: Default::default(),
            pins: Default::default(),
            garbage: Default::default(),
        };
        result.pins[1].try_wlock();
        result
    }
}

impl Global {
    /// Attempt to collect global garbage and advance the epoch
    ///
    /// Note: This may itself produce garbage.
    pub(crate) fn collect(&self, local: &Local) {
        let next = (local.epoch() + 1) % 3;
        let previous = (local.epoch() + 2) % 3;
        if self.pins[previous].try_wlock() {
            println!("{}: wlock({previous})", local.id);
            scopeguard::defer! {
                println!("{}: wunlock({next})", local.id);
                self.pins[next].wunlock();
                self.epoch.store(next, Ordering::Relaxed);
                println!("{}: epoch = {next}", local.id);
            }
            unsafe { self.garbage[next].call() }
        }
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        return;
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

        let bag = Pile::default();
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
