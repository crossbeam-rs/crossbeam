//! Tools for concurrent programming.
//!
//! ## Atomics
//!
//! * [`AtomicCell`], a thread-safe mutable memory location.
//! * [`AtomicConsume`], for reading from primitive atomic types with "consume" ordering.
//!
//! ## Data structures
//!
//! * [`deque`], work-stealing deques for building task schedulers.
//! * [`ArrayQueue`], a bounded MPMC queue that allocates a fixed-capacity buffer on construction.
//! * [`SegQueue`], an unbounded MPMC queue that allocates small buffers, segments, on demand.
//!
//! ## Memory management
//!
//! * [`epoch`], an epoch-based garbage collector.
//!
//! ## Thread synchronization
//!
//! * [`channel`], multi-producer multi-consumer channels for message passing.
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
//! [`deque`]: deque/index.html
//! [`ArrayQueue`]: queue/struct.ArrayQueue.html
//! [`SegQueue`]: queue/struct.SegQueue.html
//! [`channel`]: channel/index.html
//! [`Parker`]: sync/struct.Parker.html
//! [`ShardedLock`]: sync/struct.ShardedLock.html
//! [`WaitGroup`]: sync/struct.WaitGroup.html
//! [`epoch`]: epoch/index.html
//! [`Backoff`]: utils/struct.Backoff.html
//! [`CachePadded`]: utils/struct.CachePadded.html
//! [`scope`]: fn.scope.html

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

#[macro_use]
extern crate cfg_if;

mod _epoch {
    pub use crossbeam_epoch;
}
#[doc(inline)]
pub use crate::_epoch::crossbeam_epoch as epoch;

#[cfg_attr(
    feature = "nightly",
    cfg(all(target_has_atomic = "cas", target_has_atomic = "ptr"))
)]
pub use crossbeam_utils::atomic;

/// Miscellaneous utilities.
pub mod utils {
    pub use crossbeam_utils::Backoff;
    pub use crossbeam_utils::CachePadded;
}

cfg_if! {
    if #[cfg(feature = "std")] {
        mod _deque {
            pub use crossbeam_deque;
        }
        #[doc(inline)]
        pub use crate::_deque::crossbeam_deque as deque;

        mod _channel {
            pub use crossbeam_channel;
            pub use self::crossbeam_channel::*;
        }
        #[doc(inline)]
        pub use crate::_channel::crossbeam_channel as channel;

        // HACK(stjepang): This is the only way to reexport `select!` in Rust older than 1.30.0
        #[doc(hidden)]
        pub use crate::_channel::*;

        mod _queue {
            pub use crossbeam_queue;
        }
        #[doc(inline)]
        pub use crate::_queue::crossbeam_queue as queue;

        pub use crossbeam_utils::sync;
        pub use crossbeam_utils::thread;
        pub use crossbeam_utils::thread::scope;
    }
}
