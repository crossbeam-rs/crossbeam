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
//! [`AtomicCell`]: atomic/struct.AtomicCell.html
//! [`AtomicConsume`]: atomic/trait.AtomicConsume.html
//! [`Parker`]: sync/struct.Parker.html
//! [`ShardedLock`]: sync/struct.ShardedLock.html
//! [`WaitGroup`]: sync/struct.WaitGroup.html
//! [`Backoff`]: struct.Backoff.html
//! [`CachePadded`]: struct.CachePadded.html
//! [`scope`]: thread/fn.scope.html

#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms),
        allow(dead_code, unused_assignments, unused_variables)
    )
))]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

#[cfg_attr(feature = "nightly", cfg(target_has_atomic = "ptr"))]
pub mod atomic;

mod cache_padded;
pub use crate::cache_padded::CachePadded;

mod backoff;
pub use crate::backoff::Backoff;

use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(feature = "std")] {
        pub mod sync;
        pub mod thread;
    }
}
