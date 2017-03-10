//! Manage the thread-local state.
//!
//! This provides access to a `Participant` record.

use std::sync::atomic;

use epoch::participant::Participant;
use epoch::global;

/// A local epoch state.
///
/// The point of this type is mainly the destructor, which should be called on thread exit.
#[derive(Debug)]
struct LocalEpoch {
    /// The particpant of this thread.
    ///
    /// This pointer should never be null, but there is currently no way of expressing thread-local
    /// references ([candidate](https://github.com/rust-lang/rfcs/pull/1705)).
    participant: *const Participant,
}

// FIXME: avoid leaking when all threads have exited
impl Drop for LocalEpoch {
    fn drop(&mut self) {
        // Get the participant.
        let p = unsafe { &*self.participant };

        // Enter the active state.
        p.enter();
        // Propagate the garbage to the global state.
        p.migrate_garbage();

        // Exit the active state.
        p.exit();
        p.active.store(false, atomic::Ordering::Relaxed);
    }
}

thread_local! {
    /// The local epoch state.
    static LOCAL_EPOCH: LocalEpoch = LocalEpoch {
        // Add the current thread to the global state.
        participant: global::EPOCH.participants.enroll(),
    }
}

/// Participate in the global state.
///
/// "Participating" means that we temporarily get access to the global state, through the
/// reference, which is given to the closure.
///
/// It is relatively fast as it only accesses a thread-local variable.
pub fn with_participant<F, T>(f: F) -> T
    where F: FnOnce(&Participant) -> T {
    LOCAL_EPOCH.with(|e| f(unsafe { &*e.participant }))
}
