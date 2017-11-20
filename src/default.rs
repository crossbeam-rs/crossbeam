//! The default garbage collector.
//!
//! For each thread, a participant is lazily initialized on its first use, when the current thread
//! is registered in the default collector.  If initialized, the thread's participant will get
//! destructed on thread exit, which in turn unregisters the thread.

use collector::{Collector, Handle};
use guard::Guard;

lazy_static! {
    /// The global data for the default garbage collector.
    static ref COLLECTOR: Collector = Collector::new();
}

thread_local! {
    /// The per-thread participant for the default garbage collector.
    static HANDLE: Handle = COLLECTOR.handle();
}

/// Pins the current thread.
#[inline]
pub fn pin() -> Guard {
    // FIXME(jeehoonkang): thread-local storage may be destructed at the time `pin()` is called. For
    // that case, we should use `HANDLE.try_with()` instead.
    HANDLE.with(|handle| handle.pin())
}

/// Returns `true` if the current thread is pinned.
#[inline]
pub fn is_pinned() -> bool {
    // FIXME(jeehoonkang): thread-local storage may be destructed at the time `pin()` is called. For
    // that case, we should use `HANDLE.try_with()` instead.
    HANDLE.with(|handle| handle.is_pinned())
}

/// Returns the default handle associated with the current thread.
#[inline]
pub fn default_handle() -> Handle {
    HANDLE.with(|handle| handle.clone())
}
