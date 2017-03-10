//! RAII guards for epochs.

use std::{marker, ops};

use super::{local, Shared};

/// Pin the current epoch.
///
/// Threads generally pin before interacting with a lock-free data structure. It activates an
/// epoch, represented by a `Guard`, which can be used in types like `Atomic<T>`.
///
/// # Performance
///
/// Pinning requires a full memory barrier, so is somewhat expensive. It is however rentrant -- you can
/// safely acquire nested guards, and only the first guard requires a barrier.
///
/// In cases where you expect to perform several lock-free operations in quick succession, you may
/// consider pinning around the entire set of operations.
pub fn pin() -> Guard {
    // Set the current thread as participant while handling the epoch.
    local::with_participant(|p| {
        // Start the epoch.
        let entered = p.enter();

        // Construct the guard. This is safe as we have already called `enter`, ensuring that there
        // is an active epoch for this thread.
        let g = unsafe { Guard::fake() };

        // Collect the garbage if needed.
        if entered && p.should_gc() {
            // If it doesn't succeed, we will just wait 'till next time.
            let _ = p.try_collect(&g);
        }

        g
    })
}

/// An RAII-style guard for pinning the current epoch.
///
/// A guard must be acquired before most operations on an `Atomic` pointer. On destruction, it
/// unpins the epoch.
#[must_use]
#[derive(Debug)]
pub struct Guard {
    /// A marker to force `!Sync` and `!Send`.
    ///
    /// Sharing a guard between threads is unsafe, as it only tracks an epoch of the current
    /// thread.
    _marker: marker::PhantomData<*mut ()>,
}

impl Guard {
    /// Create a new `Shared<T>` reference.
    ///
    /// This creates a shared reference, bound to this epoch.
    pub fn new_shared<T>(&self, to: &T) -> Shared<T> {
        unsafe {
            // This is safe as the existence of the guard (`self`) for the lifetime is the
            // invariant.
            Shared::from_ptr(to)
        }
    }

    /// Assert that the value is no longer reachable.
    ///
    /// This asserts that the value is no longer possible to reach from a lock-free data structure
    /// and should be collected when sufficient epochs have passed.
    pub unsafe fn unlinked<T>(&self, val: Shared<T>) {
        // Add it for reclamation.
        local::with_participant(|p| p.reclaim(val.as_raw()))
    }

    /// Move the thread-local garbage into the global set of garbage.
    pub fn migrate_garbage(&self) {
        local::with_participant(|p| p.migrate_garbage())
    }

    /// Fakes a new guard.
    ///
    /// # Safety
    ///
    /// This relies on the caller ensuring that an epoch is entered.
    pub unsafe fn fake() -> Guard {
        Guard {
            _marker: marker::PhantomData,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Exit the epoch guard.
        local::with_participant(|p| p.exit());
    }
}

/// A value pinned to an epoch.
///
/// This wraps an arbitrary type, such that it can only be indirectly access, while also holding a
/// epoch guard, such that the value must span an epoch.
///
/// The value is accessed through it's `std::ops::Deref` implementation.
#[derive(Debug)]
pub struct Pinned<T> {
    /// The associated guard.
    guard: Guard,
    /// The inner value.
    ///
    /// Most importantly this can NEVER be accessed except for behind immutable references. This is
    /// extremely important for correctness as it is tied to the lifetime of `self.guard`.
    inner: T,
}

impl<T> Pinned<T> {
    /// Create a `Pinned<T>` based on the parameters.
    ///
    /// This ties `inner` to `guard` such that they can never be separated except when dropped.
    pub fn new(inner: T, guard: Guard) -> Pinned<T> {
        Pinned {
            guard: guard,
            inner: inner,
        }
    }

    /// Obtain the associated epoch.
    ///
    /// This allows you to access the epoch guard which is tied to the inner value.
    pub fn epoch(&self) -> &Guard {
        &self.guard
    }
}

impl<T> ops::Deref for Pinned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}
