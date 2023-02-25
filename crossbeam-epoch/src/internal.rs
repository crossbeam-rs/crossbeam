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

use crate::sync::{pile::Pile, rwlock::RwLock};
use core::cell::Cell;
use core::sync::atomic::Ordering;

use alloc::rc::Rc;
use crossbeam_utils::CachePadded;

use crate::collector::{Collector, LocalHandle};
use crate::deferred::Deferred;
use crate::primitive::sync::atomic::AtomicUsize;

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
    buffer: Cell<Vec<Deferred>>,

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
            buffer: Cell::new(vec![]),
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
        let mut buffer = self.buffer.replace(vec![]);
        buffer.push(deferred);
        let buffered = buffer.len();
        self.buffer.set(buffer);

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
            // println!("{}: rlock({})", self.id, epoch);
            self.epoch.set(epoch);
        }
    }

    /// Unpins the `Local`.
    #[inline]
    pub(crate) fn unpin(&self) {
        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count - 1);

        if guard_count == 1 {
            // println!("{}: runlock({})", self.id, self.epoch());
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
                // println!("{}: runlock({})", self.id, self.epoch());
                self.global().pins[self.epoch()].runlock(self.id);
                while !self.global().pins[new_epoch].try_rlock(self.id) {
                    new_epoch = (new_epoch + 1) % 3;
                }
                // println!("{}: rlock({})", self.id, new_epoch);
                self.epoch.set(new_epoch);
            }
        }
    }

    /// `defer()` rarely calls `flush()`, so we want the compiler to place that call on a
    /// cold path. In other words, we want the compiler to optimize branching for the case
    /// when `flush()` is not called.
    #[cold]
    pub(crate) fn flush(&self) {
        let bag = &self.global().garbage[self.epoch()];
        let mut buffer = self.buffer.replace(vec![]);
        // Safety: We are pinned to self.epoch at this point
        unsafe { bag.try_push(&mut buffer) };
        self.buffer.set(buffer);
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
            // println!("{}: wlock({previous})", local.id);
            scopeguard::defer! {
                // println!("{}: wunlock({next})", local.id);
                self.pins[next].wunlock();
                self.epoch.store(next, Ordering::Relaxed);
                // println!("{}: epoch = {next}", local.id);
            }
            unsafe { self.garbage[next].call() }
        }
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        let mut buffer = self.buffer.replace(vec![]);
        if !buffer.is_empty() {
            self.pin();
            let bag = &self.global().garbage[self.epoch()];
            while !buffer.is_empty() {
                // Safety: We are pinned to self.epoch at this point
                unsafe { bag.try_push(&mut buffer) }
            }
            self.unpin();
        }
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
}
