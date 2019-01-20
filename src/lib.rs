//! Tools for concurrent programming.
//!
//! * Atomics
//!     * [`ArcCell<T>`] is a shared mutable [`Arc<T>`] pointer.
//!     * [`AtomicCell<T>`] is equivalent to [`Cell<T>`], except it is also thread-safe.
//!     * [`AtomicConsume`] allows reading from primitive atomic types with "consume" ordering.
//!
//! * Data structures
//!     * [`deque`] module contains work-stealing deques for building task schedulers.
//!     * [`MsQueue<T>`] and [`SegQueue<T>`] are simple concurrent queues.
//!     * [`TreiberStack<T>`] is a lock-free stack.
//!
//! * Thread synchronization
//!     * [`channel`] module contains multi-producer multi-consumer channels for message passing.
//!     * [`ShardedLock<T>`] is like [`RwLock<T>`], but sharded for faster concurrent reads.
//!     * [`WaitGroup`] enables threads to synchronize the beginning or end of some computation.
//!
//! * Memory management
//!     * [`epoch`] module contains epoch-based garbage collection.
//!
//! * Utilities
//!     * [`CachePadded<T>`] pads and aligns a value to the length of a cache line.
//!     * [`scope()`] can spawn threads that borrow local variables from the stack.
//!
//! [`ArcCell<T>`]: atomic/struct.ArcCell.html
//! [`Arc<T>`]: https://doc.rust-lang.org/std/sync/struct.Arc.html
//! [`AtomicCell<T>`]: atomic/struct.AtomicCell.html
//! [`Cell<T>`]: https://doc.rust-lang.org/std/cell/struct.Cell.html
//! [`AtomicConsume`]: atomic/trait.AtomicConsume.html
//! [`deque`]: deque/index.html
//! [`MsQueue<T>`]: queue/struct.MsQueue.html
//! [`SegQueue<T>`]: queue/struct.SegQueue.html
//! [`TreiberStack<T>`]: stack/struct.TreiberStack.html
//! [`channel`]: channel/index.html
//! [`ShardedLock<T>`]: sync/struct.ShardedLock.html
//! [`RwLock<T>`]: https://doc.rust-lang.org/std/sync/struct.RwLock.html
//! [`WaitGroup`]: sync/struct.WaitGroup.html
//! [`epoch`]: epoch/index.html
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

mod arc_cell;

extern crate crossbeam_utils;

/// Atomic types.
pub mod atomic {
    pub use arc_cell::ArcCell;
    pub use crossbeam_utils::atomic::AtomicCell;
    pub use crossbeam_utils::atomic::AtomicConsume;
}

/// Miscellaneous utilities.
pub mod utils {
    pub use crossbeam_utils::CachePadded;
}

cfg_if! {
    if #[cfg(feature = "std")] {
        pub use crossbeam_utils::thread;

        // Export `crossbeam_utils::thread::scope` into the crate root because it's become an
        // established pattern.
        pub use crossbeam_utils::thread::scope;

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

        #[macro_use]
        extern crate lazy_static;
        extern crate parking_lot;

        mod ms_queue;
        mod seg_queue;
        mod sharded_lock;
        mod treiber_stack;
        mod wait_group;

        /// Concurrent queues.
        pub mod queue {
            pub use ms_queue::MsQueue;
            pub use seg_queue::SegQueue;
        }

        /// Concurrent stacks.
        pub mod stack {
            pub use treiber_stack::TreiberStack;
        }

        /// Thread synchronization primitives.
        pub mod sync {
            pub use crossbeam_utils::sync::Parker;
            pub use sharded_lock::{ShardedLock, ShardedLockReadGuard, ShardedLockWriteGuard};
            pub use wait_group::WaitGroup;
        }
    }
}
