//! Concurrent work-stealing deques.
//!
//! These data structures are most commonly used in work-stealing schedulers. The typical setup
//! involves a number of threads, each having its own FIFO or LIFO queue (*worker*). There is also
//! one global FIFO queue (*injector*) and a list of references to *worker* queues that are able to
//! steal tasks (*stealers*).
//!
//! We spawn a new task onto the scheduler by pushing it into the *injector* queue. Each worker
//! thread waits in a loop until it finds the next task to run and then runs it. To find a task, it
//! first looks into its local *worker* queue, and then into the *injector* and *stealers*.
//!
//! # Queues
//!
//! [`Injector`] is a FIFO queue, where tasks are pushed and stolen from opposite ends. It is
//! shared among threads and is usually the entry point for new tasks.
//!
//! [`Worker`] has two constructors:
//!
//! * [`new_fifo()`] - Creates a FIFO queue, in which tasks are pushed and popped from opposite
//!   ends.
//! * [`new_lifo()`] - Creates a LIFO queue, in which tasks are pushed and popped from the same
//!   end.
//!
//! Each [`Worker`] is owned by a single thread and supports only push and pop operations.
//!
//! Method [`stealer()`] creates a [`Stealer`] that may be shared among threads and can only steal
//! tasks from its [`Worker`]. Tasks are stolen from the end opposite to where they get pushed.
//!
//! # Stealing
//!
//! Steal operations come in three flavors:
//!
//! 1. [`steal()`] - Steals one task.
//! 2. [`steal_batch()`] - Steals a batch of tasks and moves them into another worker.
//! 3. [`steal_batch_and_pop()`] - Steals a batch of tasks, moves them into another queue, and pops
//!    one task from that worker.
//!
//! In contrast to push and pop operations, stealing can spuriously fail with [`Steal::Retry`], in
//! which case the steal operation needs to be retried.
//!
//! # Examples
//!
//! Suppose a thread in a work-stealing scheduler is idle and looking for the next task to run. To
//! find an available task, it might do the following:
//!
//! 1. Try popping one task from the local worker queue.
//! 2. Try stealing a batch of tasks from the global injector queue.
//! 3. Try stealing one task from another thread using the stealer list.
//!
//! An implementation of this work-stealing strategy:
//!
//! ```
//! use crossbeam_deque::{Injector, Stealer, Worker};
//! use std::iter;
//!
//! fn find_task<T>(
//!     local: &Worker<T>,
//!     global: &Injector<T>,
//!     stealers: &[Stealer<T>],
//! ) -> Option<T> {
//!     // Pop a task from the local queue, if not empty.
//!     local.pop().or_else(|| {
//!         // Otherwise, we need to look for a task elsewhere.
//!         iter::repeat_with(|| {
//!             // Try stealing a batch of tasks from the global queue.
//!             global.steal_batch_and_pop(local)
//!                 // Or try stealing a task from one of the other threads.
//!                 .or_else(|| stealers.iter().map(|s| s.steal()).collect())
//!         })
//!         // Loop while no task was stolen and any steal operation needs to be retried.
//!         .find(|s| !s.is_retry())
//!         // Extract the stolen task, if there is one.
//!         .and_then(|s| s.success())
//!     })
//! }
//! ```
//!
//! [`new_fifo()`]: Worker::new_fifo
//! [`new_lifo()`]: Worker::new_lifo
//! [`stealer()`]: Worker::stealer
//! [`steal()`]: Stealer::steal
//! [`steal_batch()`]: Stealer::steal_batch
//! [`steal_batch_and_pop()`]: Stealer::steal_batch_and_pop

#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms),
        allow(dead_code, unused_assignments, unused_variables)
    )
))]
#![warn(
    missing_docs,
    missing_debug_implementations,
    rust_2018_idioms,
    unreachable_pub
)]
#![allow(clippy::question_mark)] // https://github.com/rust-lang/rust-clippy/issues/8281
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(crossbeam_loom)]
extern crate loom_crate as loom;

use cfg_if::cfg_if;

#[cfg(crossbeam_loom)]
#[allow(unused_imports, dead_code)]
mod primitive {
    pub(crate) mod cell {
        pub(crate) use loom::cell::UnsafeCell;
    }
    pub(crate) mod sync {
        pub(crate) mod atomic {
            pub(crate) use loom::sync::atomic::{
                fence, AtomicIsize, AtomicPtr, AtomicUsize, Ordering,
            };

            // FIXME: loom does not support compiler_fence at the moment.
            // https://github.com/tokio-rs/loom/issues/117
            // we use fence as a stand-in for compiler_fence for the time being.
            // this may miss some races since fence is stronger than compiler_fence,
            // but it's the best we can do for the time being.
            pub(crate) use self::fence as compiler_fence;
        }
        pub(crate) use loom::sync::Arc;
    }
    pub(crate) use loom::thread_local;
}
#[cfg(not(crossbeam_no_atomic_cas))]
#[cfg(not(crossbeam_loom))]
#[allow(unused_imports, dead_code)]
mod primitive {
    pub(crate) mod cell {
        #[derive(Debug)]
        #[repr(transparent)]
        pub(crate) struct UnsafeCell<T>(::core::cell::UnsafeCell<T>);

        // loom's UnsafeCell has a slightly different API than the standard library UnsafeCell.
        // Since we want the rest of the code to be agnostic to whether it's running under loom or
        // not, we write this small wrapper that provides the loom-supported API for the standard
        // library UnsafeCell. This is also what the loom documentation recommends:
        // https://github.com/tokio-rs/loom#handling-loom-api-differences
        impl<T> UnsafeCell<T> {
            #[inline]
            pub(crate) fn new(data: T) -> UnsafeCell<T> {
                UnsafeCell(::core::cell::UnsafeCell::new(data))
            }

            #[inline]
            pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
                f(self.0.get())
            }

            #[inline]
            pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
                f(self.0.get())
            }
        }
    }
    pub(crate) mod sync {
        pub(crate) mod atomic {
            pub(crate) use core::sync::atomic::{
                compiler_fence, fence, AtomicIsize, AtomicPtr, AtomicUsize, Ordering,
            };
        }
        #[cfg(feature = "std")]
        pub(crate) use std::sync::Arc;
    }

    #[cfg(feature = "std")]
    pub(crate) use std::thread_local;
}

cfg_if! {
    if #[cfg(feature = "std")] {
        use crossbeam_epoch as epoch;
        use crossbeam_utils as utils;

        mod deque;
        pub use crate::deque::{Injector, Steal, Stealer, Worker};
    }
}
