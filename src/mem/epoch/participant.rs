// Manages a single participant in the epoch scheme. This is where all
// of the actual epoch management logic happens!

use std::mem;
use std::cell::UnsafeCell;
use std::fmt;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};

use mem::epoch::{Atomic, Guard, garbage, global};
use mem::epoch::participants::ParticipantNode;

/// Thread-local data for epoch participation.
pub struct Participant {
    /// The local epoch.
    epoch: AtomicUsize,

    /// Number of pending uses of `epoch::pin()`; keeping a count allows for
    /// reentrant use of epoch management.
    in_critical: AtomicUsize,

    /// Thread-local garbage tracking
    garbage: UnsafeCell<garbage::Local>,

    /// Is the thread still active? Becomes `false` when the thread exits. This
    /// is ultimately used to free `Participant` records.
    pub active: AtomicBool,

    /// The participant list is coded intrusively; here's the `next` pointer.
    pub next: Atomic<ParticipantNode>,
}

impl fmt::Debug for Participant {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Participant {{ ... }}")
    }
}

unsafe impl Sync for Participant {}

const GC_THRESH: usize = 32;

impl Participant {
    pub fn new() -> Participant {
        Participant {
            epoch: AtomicUsize::new(0),
            in_critical: AtomicUsize::new(0),
            active: AtomicBool::new(true),
            garbage: UnsafeCell::new(garbage::Local::new()),
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
            unsafe { (*self.garbage.get()).collect(); }
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
        (*self.garbage.get()).insert(data);
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

        unsafe {
            (*self.garbage.get()).collect();
            global::get().garbage[new_epoch.wrapping_add(1) % 3].collect();
        }
        self.epoch.store(new_epoch, Release);

        true
    }

    /// Move the current thread-local garbage into the global garbage bags.
    pub fn migrate_garbage(&self) {
        let cur_epoch = self.epoch.load(Relaxed);
        let local = unsafe { mem::replace(&mut *self.garbage.get(), garbage::Local::new()) };
        global::get().garbage[cur_epoch.wrapping_sub(1) % 3].insert(local.old);
        global::get().garbage[cur_epoch % 3].insert(local.cur);
        global::get().garbage[global::get().epoch.load(Relaxed) % 3].insert(local.new);
    }

    /// How much garbage is this participant currently storing?
    pub fn garbage_size(&self) -> usize {
        unsafe { (*self.garbage.get()).size() }
    }
    /// Is this participant past its local GC threshhold?
    pub fn should_gc(&self) -> bool {
        self.garbage_size() >= GC_THRESH
    }
}
