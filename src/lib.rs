//! Crossbeam supports concurrent programming, especially focusing on memory
//! management, synchronization, and non-blocking data structures.
//!
//! Crossbeam consists of several submodules:
//!
//!  - `atomic` for **enhancing `std::sync` API**. `AtomicConsume` provides
//!    C/C++11-style "consume" atomic operations (re-exported from
//!    [`crossbeam-utils`]). `ArcCell` provides atomic storage and retrieval of
//!    `Arc`.
//!
//!  - `utils` and `thread` for **utilities**, re-exported from [`crossbeam-utils`].
//!    The "scoped" thread API in `thread` makes it possible to spawn threads that
//!    share stack data with their parents. The `utils::CachePadded` struct inserts
//!    padding to align data with the size of a cacheline. This crate also seeks to
//!    expand the standard library's few synchronization primitives (locks,
//!    barriers, etc) to include advanced/niche primitives, as well as userspace
//!    alternatives.
//!
//!  - `epoch` for **memory management**, re-exported from [`crossbeam-epoch`].
//!    Because non-blocking data structures avoid global synchronization, it is not
//!    easy to tell when internal data can be safely freed. The crate provides
//!    generic, easy to use, and high-performance APIs for managing memory in these
//!    cases. We plan to support other memory management schemes, e.g. hazard
//!    pointers (HP) and quiescent state-based reclamation (QSBR).
//!
//!  - **Concurrent data structures** which are non-blocking and much superior to
//!    wrapping sequential ones with a `Mutex`. Crossbeam currently provides
//!    channels (re-exported from [`crossbeam-channel`]), deques
//!    (re-exported from [`crossbeam-deque`]), queues, and stacks. Ultimately the
//!    goal is to also include bags, sets and maps.

#![warn(missing_docs)]
// #![warn(missing_debug_implementations)] // TODO: Uncomment this.
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
pub use _epoch::crossbeam_epoch as epoch;

mod arc_cell;
mod atomic_cell;

extern crate crossbeam_utils;

/// Additional utilities for atomics.
pub mod atomic {
    pub use arc_cell::ArcCell;
    pub use atomic_cell::AtomicCell;
    pub use crossbeam_utils::atomic::AtomicConsume;
}

/// Utilities for concurrent programming.
///
/// See [the `crossbeam-utils` crate](https://github.com/crossbeam-rs/crossbeam-utils) for more
/// information.
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
        pub use _deque::crossbeam_deque as deque;

        mod _channel {
            pub extern crate crossbeam_channel;
            pub use self::crossbeam_channel::*;
        }
        pub use _channel::crossbeam_channel as channel;

        // HACK(stjepang): This is the only way to reexport `select!` in Rust older than 1.30.0
        #[doc(hidden)]
        pub use _channel::*;

        #[macro_use]
        extern crate lazy_static;
        extern crate num_cpus;
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

        /// Utilities for thread synchronization.
        pub mod sync {
            pub use sharded_lock::{ShardedLock, ShardedLockReadGuard, ShardedLockWriteGuard};
            pub use wait_group::WaitGroup;
        }
    }
}
