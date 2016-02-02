// Manages a single participant in the epoch scheme. This is where all
// of the actual epoch management logic happens!

use std::cell::Cell;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};

use mem::epoch::{Atomic, Guard, global};
use mem::epoch::participants::ParticipantNode;

/// Thread-local data for epoch participation.
pub struct Participant {
    /// The local epoch.
    epoch: AtomicUsize,

    /// Number of pending uses of `epoch::pin()`; keeping a count allows for
    /// reentrant use of epoch management.
    in_critical: AtomicUsize,

    /// Number of operations since last GC
    num_ops: Cell<u64>,

    /// Is the thread still active? Becomes `false` when the thread exits. This
    /// is ultimately used to free `Participant` records.
    pub active: AtomicBool,

    /// The participant list is coded intrusively; here's the `next` pointer.
    pub next: Atomic<ParticipantNode>,
}

unsafe impl Sync for Participant {}

const GC_THRESH: u64 = 128;

impl Participant {
    pub fn new() -> Participant {
        Participant {
            epoch: AtomicUsize::new(0),
            in_critical: AtomicUsize::new(0),
            num_ops: Cell::new(0),
            active: AtomicBool::new(true),
            next: Atomic::null(),
        }
    }

    /// Enter a critical section.
    ///
    /// This method is reentrant, allowing for nested critical sections.
    ///
    /// Returns `true` is this is the first entry on the stack (as opposed to a
    /// re-entrant call).
    pub fn enter(&self) -> bool {
        let new_count = self.in_critical.load(Relaxed) + 1;
        self.in_critical.store(new_count, Relaxed);
        if new_count > 1 { return false }

        atomic::fence(SeqCst);

        let global_epoch = global::get().epoch.load(Relaxed);
        if global_epoch != self.epoch.load(Relaxed) {
            self.epoch.store(global_epoch, Relaxed);
            self.num_ops.set(0);
        } else {
            self.num_ops.set(self.num_ops.get() + 1);
        }

        true
    }

    /// Exit the current (nested) critical section.
    pub fn exit(&self) {
        let new_count = self.in_critical.load(Relaxed) - 1;
        self.in_critical.store(
            new_count,
            if new_count > 0 { Relaxed } else { Release });
    }

    /// Begin the reclamation process for a piece of data.
    pub unsafe fn reclaim<T>(&self, data: *mut T) {
        let cur_global = global::get().epoch.load(Acquire);
        global::get().garbage[cur_global % 3].insert(data);
    }

    /// Attempt to collect garbage by moving the global epoch forward.
    ///
    /// Returns `true` on success.
    pub fn try_collect(&self, guard: &Guard) -> bool {
        let cur_epoch = global::get().epoch.load(SeqCst);

        for p in global::get().participants.iter(guard) {
            if p.in_critical.load(Relaxed) > 0 && p.epoch.load(Relaxed) != cur_epoch {
                return false
            }
        }

        let new_epoch = cur_epoch.wrapping_add(1);
        atomic::fence(Acquire);
        if global::get().epoch.compare_and_swap(cur_epoch, new_epoch, SeqCst) != cur_epoch {
            return false
        }

        self.epoch.store(new_epoch, Relaxed);
        unsafe {
            global::get().garbage[new_epoch.wrapping_add(1) % 3].collect();
        }
        self.num_ops.set(0);

        true
    }

    /// Is this participant past its local GC threshhold?
    pub fn should_gc(&self) -> bool {
        self.num_ops.get() >= GC_THRESH
    }
}
