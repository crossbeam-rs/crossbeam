// Manages a single participant in the epoch scheme. This is where all
// of the actual epoch management logic happens!

use std::{fmt, mem};
use std::cell::UnsafeCell;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};

use mem::epoch::{Atomic, Guard, garbage, global};
use mem::epoch::participants::ParticipantNode;

/// The garbage collection threshold (in garbage items).
const GC_THRESHOLD: usize = 32;

/// Thread-local data for epoch participation.
pub struct Participant {
    /// The local epoch.
    ///
    /// This is incremented on new epochs.
    epoch: AtomicUsize,
    /// Number of pending uses of `epoch::pin()`.
    ///
    /// Keeping a count allows for reentrant use of epoch management.
    in_critical: AtomicUsize,
    /// The thread-local garbage tracking.
    garbage: UnsafeCell<garbage::Local>,
    /// Is the thread still active?
    ///
    /// This becomes `false` when the thread exits. This is ultimately used to free `Participant`
    /// records.
    pub active: AtomicBool,
    /// The next participant node.
    ///
    /// The participant list is coded intrusively; here's the pointer to the next node.
    pub next: Atomic<ParticipantNode>,
}

impl Participant {
    /// Enter a critical section.
    ///
    /// This method is reentrant, allowing for nested critical sections.
    ///
    /// It returns `true` is this is the first entry on the stack (as opposed to a re-entrant
    /// call).
    pub fn enter(&self) -> bool {
        // Increment the counter.
        if self.in_critical.fetch_add(1, atomic::Ordering::Relaxed) > 0 {
            // Since it was above zero, we return false.
            false
        } else {
            // This fence is critical to avoid reordering with the increment.
            atomic::fence(atomic::Ordering::SeqCst);

            // Load the global epoch counter and test if this epoch matches the global.
            let global_epoch = global::EPOCH.epoch.load(atomic::Ordering::Relaxed);
            if global_epoch != self.epoch.load(atomic::Ordering::Relaxed) {
                // As it isn't already at the global epoch count, set it.
                self.epoch.store(global_epoch, atomic::Ordering::Relaxed);

                // Then collect the garbage.
                unsafe {
                    (*self.garbage.get()).collect();
                }
            }

            true
        }
    }

    /// Exit the current (nested) critical section.
    pub fn exit(&self) {
        // Decrement the counter.
        self.in_critical.fetch_sub(1, atomic::Ordering::SeqCst);
    }

    /// Begin the reclamation process for a piece of data.
    pub unsafe fn reclaim<T>(&self, data: *mut T) {
        // Insert it into the garbage list.
        (*self.garbage.get()).insert(data);
    }

    /// Attempt to collect garbage by moving the global epoch forward.
    ///
    /// If it failed, it returns `Err(())`.
    pub fn try_collect(&self, guard: &Guard) -> Result<(), ()> {
        // Load the global epoch counter.
        let cur_epoch = global::EPOCH.epoch.load(atomic::Ordering::SeqCst);

        // Go over the participants to ensure that none are critical.
        for p in global::EPOCH.participants.iter(guard) {
            if p.is_critical() && p.epoch.load(atomic::Ordering::Relaxed) != cur_epoch {
                // A participant was in critical state, and can thus not be GC'd at this moment.
                return Err(());
            }
        }

        let new_epoch = cur_epoch.wrapping_add(1);

        // This fence is important to avoid reordering of the CAS with any of the previous loads.
        // TODO: Is this really necessary?
        atomic::fence(atomic::Ordering::Acquire);
        // CAS with the new epoch. The reason we do CAS and not a simple store is that we want to
        // solve the ABA problem, as another thread could have updated the counter, causing us to
        // reverting its changes. Hence, we must ensure that this is NOT the case.
        if global::EPOCH.epoch.compare_and_swap(cur_epoch, new_epoch, atomic::Ordering::SeqCst) != cur_epoch {
            // Another thread touched it, so the collection failed.
            return Err(());
        }

        // If you've come so far, everything is right for collecting the garbage, so let's do it.
        unsafe {
            (*self.garbage.get()).collect();
            global::EPOCH.garbage.collect(new_epoch.wrapping_add(1))
        }

        // Update the epoch counter. As the epoch was collected, the ABA problem shouldn't be an
        // issue here.
        // TODO: Is this really correct?
        self.epoch.store(new_epoch, atomic::Ordering::Release);

        Ok(())
    }

    /// Move the current thread-local garbage into the global garbage bags.
    pub fn migrate_garbage(&self) {
        // Load the current epoch counter.
        let cur_epoch = self.epoch.load(atomic::Ordering::Relaxed);
        // Replace the local garbage set with an empty set.
        let local = unsafe { mem::replace(&mut *self.garbage.get(), garbage::Local::default()) };
        // Insert each epoch-kind into the global set.
        global::EPOCH.garbage.insert(cur_epoch.wrapping_sub(1), local.old);
        global::EPOCH.garbage.insert(cur_epoch, local.cur);
        global::EPOCH.garbage.insert(cur_epoch.wrapping_add(1), local.new);

        // TODO
        // global::EPOCH.garbage[global::EPOCH.epoch.load(atomic::Ordering::Relaxed) % 3].insert(local.new);
    }

    /// How many garbage items is this participant currently storing?
    pub fn garbage_len(&self) -> usize {
        unsafe { (*self.garbage.get()).len() }
    }

    /// Is this participant past its local GC threshold?
    pub fn should_gc(&self) -> bool {
        // We only GC when the garbage length is above some threshold.
        self.garbage_len() >= GC_THRESHOLD
    }

    /// Is this participant in a critical state?
    ///
    /// "Critical state" means that an `epoch::pin()` is pending, and thus it cannot be modified
    /// ATM.
    fn is_critical(&self) -> bool {
        self.in_critical.load(atomic::Ordering::Relaxed) > 0
    }
}

unsafe impl Sync for Participant {}

impl fmt::Debug for Participant {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Participant {{ ... }}")
    }
}

impl Default for Participant {
    fn default() -> Participant {
        Participant {
            epoch: AtomicUsize::new(0),
            in_critical: AtomicUsize::new(0),
            active: AtomicBool::new(true),
            garbage: UnsafeCell::new(garbage::Local::default()),
            next: Atomic::null(),
        }
    }
}
