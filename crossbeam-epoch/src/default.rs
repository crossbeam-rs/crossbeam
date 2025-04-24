//! The default garbage collector.
//!
//! For each thread, a participant is lazily initialized on its first use, when the current thread
//! is registered in the default collector.  If initialized, the thread's participant will get
//! destructed on thread exit, which in turn unregisters the thread.

use crate::collector::{Collector, LocalHandle};
use crate::guard::Guard;
use crate::primitive::thread_local;

// Based on unstable std::sync::OnceLock.
//
// Source: https://github.com/rust-lang/rust/blob/8e9c93df464b7ada3fc7a1c8ccddd9dcb24ee0a0/library/std/src/sync/once_lock.rs

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use std::sync::Once;

pub(crate) struct OnceLock<T> {
    once: Once,
    value: UnsafeCell<MaybeUninit<T>>,
    // Unlike std::sync::OnceLock, we don't need PhantomData here because
    // we don't use #[may_dangle].
}

unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

impl<T> OnceLock<T> {
    /// Creates a new empty cell.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            once: Once::new(),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Gets the contents of the cell, initializing it with `f` if the cell
    /// was empty.
    ///
    /// Many threads may call `get_or_init` concurrently with different
    /// initializing functions, but it is guaranteed that only one function
    /// will be executed.
    ///
    /// # Panics
    ///
    /// If `f` panics, the panic is propagated to the caller, and the cell
    /// remains uninitialized.
    ///
    /// It is an error to reentrantly initialize the cell from `f`. The
    /// exact outcome is unspecified. Current implementation deadlocks, but
    /// this may be changed to a panic in the future.
    pub(crate) fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        // Fast path check
        if self.once.is_completed() {
            // SAFETY: The inner value has been initialized
            return unsafe { self.get_unchecked() };
        }
        self.initialize(f);

        // SAFETY: The inner value has been initialized
        unsafe { self.get_unchecked() }
    }

    #[cold]
    fn initialize<F>(&self, f: F)
    where
        F: FnOnce() -> T,
    {
        let slot = self.value.get();

        self.once.call_once(|| {
            let value = f();
            unsafe { slot.write(MaybeUninit::new(value)) }
        });
    }

    /// # Safety
    ///
    /// The value must be initialized
    unsafe fn get_unchecked(&self) -> &T {
        debug_assert!(self.once.is_completed());
        unsafe { (*self.value.get()).assume_init_ref() }
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.once.is_completed() {
            // SAFETY: The inner value has been initialized
            // assume_init_drop requires Rust 1.60
            // unsafe { (*self.value.get()).assume_init_drop() };
            unsafe { self.value.get().cast::<T>().drop_in_place() };
        }
    }
}


#[cfg(not(crossbeam_loom))]
//use crate::sync::once_lock::OnceLock;

fn collector() -> &'static Collector {
    #[cfg(not(crossbeam_loom))]
    {
        /// The global data for the default garbage collector.
        static COLLECTOR: OnceLock<Collector> = OnceLock::new();
        COLLECTOR.get_or_init(Collector::new)
    }
    // FIXME: loom does not currently provide the equivalent of Lazy:
    // https://github.com/tokio-rs/loom/issues/263
    #[cfg(crossbeam_loom)]
    {
        loom::lazy_static! {
            /// The global data for the default garbage collector.
            static ref COLLECTOR: Collector = Collector::new();
        }
        &COLLECTOR
    }
}

thread_local! {
    /// The per-thread participant for the default garbage collector.
    static HANDLE: LocalHandle = collector().register();
}

/// Pins the current thread.
#[inline]
pub fn pin() -> Guard {
    with_handle(|handle| handle.pin())
}

/// Returns `true` if the current thread is pinned.
#[inline]
pub fn is_pinned() -> bool {
    with_handle(|handle| handle.is_pinned())
}

/// Returns the default global collector.
pub fn default_collector() -> &'static Collector {
    collector()
}

#[inline]
fn with_handle<F, R>(mut f: F) -> R
where
    F: FnMut(&LocalHandle) -> R,
{
    HANDLE
        .try_with(|h| f(h))
        .unwrap_or_else(|_| f(&collector().register()))
}

#[cfg(all(test, not(crossbeam_loom)))]
mod tests {
    use crossbeam_utils::thread;

    #[test]
    fn pin_while_exiting() {
        struct Foo;

        impl Drop for Foo {
            fn drop(&mut self) {
                // Pin after `HANDLE` has been dropped. This must not panic.
                super::pin();
            }
        }

        std::thread_local! {
            static FOO: Foo = const { Foo };
        }

        thread::scope(|scope| {
            scope.spawn(|_| {
                // Initialize `FOO` and then `HANDLE`.
                FOO.with(|_| ());
                super::pin();
                // At thread exit, `HANDLE` gets dropped first and `FOO` second.
            });
        })
        .unwrap();
    }
}
