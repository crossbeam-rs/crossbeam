//! Tools for concurrent programming.
//!
//! * Atomics
//!     * [`AtomicCell<T>`] is equivalent to [`Cell<T>`], except it is also thread-safe.
//!     * [`AtomicConsume`] allows reading from primitive atomic types with "consume" ordering.
//!
//! * Data structures
//!     * [`deque`] module contains work-stealing deques for building task schedulers.
//!     * [`ArrayQueue<T>`] is a bounded MPMC queue.
//!     * [`SegQueue<T>`] is an unbounded MPMC queue.
//!
//! * Memory management
//!     * [`epoch`] module contains epoch-based garbage collection.
//!
//! * Thread synchronization
//!     * [`channel`] module contains multi-producer multi-consumer channels for message passing.
//!     * [`ShardedLock<T>`] is like [`RwLock<T>`], but sharded for faster concurrent reads.
//!     * [`WaitGroup`] enables threads to synchronize the beginning or end of some computation.
//!
//! * Utilities
//!     * [`Backoff`] performs exponential backoff in spin loops.
//!     * [`CachePadded<T>`] pads and aligns a value to the length of a cache line.
//!     * [`scope()`] can spawn threads that borrow local variables from the stack.
//!
//! [`Arc<T>`]: https://doc.rust-lang.org/std/sync/struct.Arc.html
//! [`AtomicCell<T>`]: atomic/struct.AtomicCell.html
//! [`Cell<T>`]: https://doc.rust-lang.org/std/cell/struct.Cell.html
//! [`AtomicConsume`]: atomic/trait.AtomicConsume.html
//! [`deque`]: deque/index.html
//! [`ArrayQueue<T>`]: queue/struct.ArrayQueue.html
//! [`SegQueue<T>`]: queue/struct.SegQueue.html
//! [`channel`]: channel/index.html
//! [`ShardedLock<T>`]: sync/struct.ShardedLock.html
//! [`RwLock<T>`]: https://doc.rust-lang.org/std/sync/struct.RwLock.html
//! [`WaitGroup`]: sync/struct.WaitGroup.html
//! [`epoch`]: epoch/index.html
//! [`Backoff`]: utils/struct.CachePadded.html
//! [`CachePadded<T>`]: utils/struct.CachePadded.html
//! [`scope()`]: fn.scope.html

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(alloc))]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]
#![cfg_attr(feature = "nightly", feature(integer_atomics))]

#[macro_use]
extern crate cfg_if;
#[cfg(feature = "std")]
extern crate core;

cfg_if! {
    if #[cfg(feature = "nightly")] {
        extern crate alloc;
    } else {
        mod alloc {
            extern crate std;
            pub use self::std::*;
        }
    }
}

mod _epoch {
    pub extern crate crossbeam_epoch;
}
#[doc(inline)]
pub use _epoch::crossbeam_epoch as epoch;

extern crate crossbeam_utils;

/// Atomic types.
pub mod atomic {
    pub use crossbeam_utils::atomic::AtomicCell;
    pub use crossbeam_utils::atomic::AtomicConsume;
}

/// Miscellaneous utilities.
pub mod utils {
    pub use crossbeam_utils::Backoff;
    pub use crossbeam_utils::CachePadded;
}

cfg_if! {
    if #[cfg(feature = "std")] {
        mod _deque {
            pub extern crate crossbeam_deque;
        }
        #[doc(inline)]
        pub use _deque::crossbeam_deque as deque;

        mod _channel {
            pub extern crate crossbeam_channel;
            pub use self::crossbeam_channel::*;
        }
        #[doc(inline)]
        pub use _channel::crossbeam_channel as channel;

        // HACK(stjepang): This is the only way to reexport `select!` in Rust older than 1.30.0
        #[doc(hidden)]
        pub use _channel::*;

        mod _queue {
            pub extern crate crossbeam_queue;
        }
        #[doc(inline)]
        pub use _queue::crossbeam_queue as queue;

        pub use crossbeam_utils::sync;
        pub use crossbeam_utils::thread;
        pub use crossbeam_utils::thread::scope;
    }
}
