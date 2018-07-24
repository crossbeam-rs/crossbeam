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

extern crate crossbeam_channel;
extern crate crossbeam_deque;
extern crate crossbeam_epoch;
extern crate crossbeam_utils;

mod arc_cell;
mod ms_queue;
mod seg_queue;
mod treiber_stack;


/// Additional utilities for atomics.
pub mod atomic {
    pub use crossbeam_utils::AtomicConsume;
    pub use arc_cell::ArcCell;
}

/// Utilities for concurrent programming.
///
/// See [the `crossbeam-utils` crate](https://github.com/crossbeam-rs/crossbeam-utils) for more
/// information.
pub mod utils {
    pub use crossbeam_utils::CachePadded;
}

// Export `crossbeam_utils::thread` and `crossbeam_utils::thread::scope` in the crate root, in order
// not to break established patterns.
pub use crossbeam_utils::thread;
pub use crossbeam_utils::thread::scope;


/// Epoch-based memory reclamation.
///
/// See [the `crossbeam-epoch` crate](https://github.com/crossbeam-rs/crossbeam-epoch) for more
/// information.
pub mod epoch {
    pub use crossbeam_epoch::*;
}


/// Multi-producer multi-consumer channels for message passing.
///
/// See [the `crossbeam-channel` crate](https://github.com/crossbeam-rs/crossbeam-channel) for more
/// information.
///
/// # Example
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam;
/// # fn main() {
/// use std::thread;
/// use crossbeam::channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// thread::spawn(move || s1.send("foo"));
/// thread::spawn(move || s2.send("bar"));
///
/// // Only one of these two receive operations will be executed.
/// select! {
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
///     recv(r2, msg) => assert_eq!(msg, Some("bar")),
/// }
/// # }
/// ```
pub mod channel {
    pub use crossbeam_channel::*;
}

// FIXME(jeehoonkang): The entirety of `crossbeam_channel::*` is re-exported as public in the crate
// root because it seems it's the only way to re-export its `select!` macro.  We need to find a more
// precise way to re-export only the `select!` macro.
#[doc(hidden)]
pub use crossbeam_channel::*;


/// A concurrent work-stealing deque.
///
/// See [the `crossbeam-deque` crate](https://github.com/crossbeam-rs/crossbeam-deque) for more
/// information.
pub mod deque {
    pub use crossbeam_deque::*;
}

/// Concurrent queues.
pub mod queue {
    pub use ms_queue::MsQueue;
    pub use seg_queue::SegQueue;
}

/// Concurrent stacks.
pub mod stack {
    pub use treiber_stack::TreiberStack;
}
