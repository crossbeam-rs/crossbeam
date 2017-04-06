//! RAII guards for epochs.

use std::marker;

use super::{local, Shared};

/// An RAII-style guard for pinning the current epoch.
///
/// A guard must be acquired before most operations on an `Atomic` pointer. On
/// destruction, it unpins the epoch.
#[must_use]
#[derive(Debug)]
pub struct Guard {
    /// A marker to force `!Sync` and `!Send`.
    ///
    /// Sharing a guard between threads is unsafe, as it only tracks an epoch
    /// of the current thread.
    _marker: marker::PhantomData<*mut ()>,
}

/// Pin the current epoch.
///
/// Threads generally pin before interacting with a lock-free data structure. It
/// activates an epoch, represented by a `Guard`, which can be used in types like
/// `Atomic<T>`.
///
/// # Performance
///
/// Pinning requires a full memory barrier, so is somewhat expensive. It is however
/// rentrant -- you can safely acquire nested guards, and only the first guard
/// requires a barrier.  Thus, in cases where you expect to perform several
/// lock-free operations in quick succession, you may consider pinning around the
/// entire set of operations.
pub fn pin() -> Guard {
    // Set the current thread as participant while handling the epoch.
    local::with_participant(|p| {
        let entered = p.enter();

        // Construct the guard. This is safe as we have already called  `enter`,
        // ensuring that there is an active epoch for this thread.
        let g = Guard {
            _marker: marker::PhantomData,
        };

        if entered && p.should_gc() {
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
