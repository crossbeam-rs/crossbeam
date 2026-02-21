//! Miscellaneous tools for concurrent programming.
//!
//! ## Atomics
//!
//! * [`AtomicCell`], a thread-safe mutable memory location.
//! * [`AtomicConsume`], for reading from primitive atomic types with "consume" ordering.
//!
//! ## Thread synchronization
//!
//! * [`Parker`], a thread parking primitive.
//! * [`ShardedLock`], a sharded reader-writer lock with fast concurrent reads.
//! * [`WaitGroup`], for synchronizing the beginning or end of some computation.
//!
//! ## Utilities
//!
//! * [`Backoff`], for exponential backoff in spin loops.
//! * [`CachePadded`], for padding and aligning a value to the length of a cache line.
//! * [`scope`], for spawning threads that borrow local variables from the stack.
//!
//! [`AtomicCell`]: atomic::AtomicCell
//! [`AtomicConsume`]: atomic::AtomicConsume
//! [`Parker`]: sync::Parker
//! [`ShardedLock`]: sync::ShardedLock
//! [`WaitGroup`]: sync::WaitGroup
//! [`scope`]: thread::scope

#![no_std]
#![doc(test(
    no_crate_inject,
    attr(allow(dead_code, unused_assignments, unused_variables))
))]
#![warn(
    missing_docs,
    unsafe_op_in_unsafe_fn,
    clippy::alloc_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "std")]
#[cfg(not(crossbeam_loom))]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(crossbeam_loom)]
#[allow(unused_imports)]
mod primitive {
    pub(crate) mod hint {
        pub(crate) use loom::hint::spin_loop;
    }
    pub(crate) mod sync {
        pub(crate) mod atomic {
            pub(crate) use loom::sync::atomic::{
                AtomicBool, AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicU16,
                AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
            };

            // FIXME: loom does not support compiler_fence at the moment.
            // https://github.com/tokio-rs/loom/issues/117
            // we use fence as a stand-in for compiler_fence for the time being.
            // this may miss some races since fence is stronger than compiler_fence,
            // but it's the best we can do for the time being.
            pub(crate) use loom::sync::atomic::fence as compiler_fence;
        }
        pub(crate) use loom::sync::{Arc, Condvar, Mutex};
    }
}
#[cfg(not(crossbeam_loom))]
#[allow(unused_imports)]
mod primitive {
    pub(crate) mod hint {
        pub(crate) use core::hint::spin_loop;
    }
    pub(crate) mod sync {
        #[cfg(feature = "std")]
        pub(crate) use alloc::sync::Arc;
        pub(crate) use core::sync::atomic;
        #[cfg(feature = "std")]
        pub(crate) use std::sync::{Condvar, Mutex};
    }
}

#[cfg(feature = "atomic")]
#[cfg_attr(docsrs, doc(cfg(feature = "atomic")))]
pub mod atomic;

mod cache_padded;
pub use crate::cache_padded::CachePadded;

mod backoff;
pub use crate::backoff::Backoff;

#[cfg(feature = "std")]
pub mod sync;

#[cfg(feature = "std")]
#[cfg(not(crossbeam_loom))]
pub mod thread;
