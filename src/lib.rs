//! Support for concurrent programming: memory management, synchronization,
//! non-blocking data structures.
//!
//! Crossbeam consists of several subcrates.
//!
//! - `crossbeam-epoch` for **Memory management**. Because non-blocking data
//!   structures avoid global synchronization, it is not easy to tell when
//!   internal data can be safely freed. The crate provides generic, easy to
//!   use, and high-performance APIs for managing memory in these cases. We plan
//!   to support other memory management schemes, e.g. hazard pointers (HP) and
//!   quiescent state-based reclamation (QSBR). The crate is reexported as the
//!   `epoch` module.
//!
//! - `crossbeam-utils` for **Utilities**. The "scoped" thread API makes it
//!   possible to spawn threads that share stack data with their parents. The
//!   `CachePadded` struct inserts padding to align data with the size of a
//!   cacheline. This crate also seeks to expand the standard library's few
//!   synchronization primitives (locks, barriers, etc) to include
//!   advanced/niche primitives, as well as userspace alternatives. This crate
//!   is reexported as the `utils` module. `CachePadded` and scoped thread API
//!   are also reexported at the top-level.
//!
//! - **Non-blocking data structures**. Several crates provide high performance
//!   and highly-concurrent data structures, which are much superior to wrapping
//!   with a `Mutex`. Ultimately the goal is to include stacks, queues, deques,
//!   bags, sets and maps. These subcrates are reexported in the `sync` module.

#![warn(missing_docs)]

extern crate crossbeam_epoch;
extern crate crossbeam_utils;
extern crate crossbeam_channel;
extern crate crossbeam_deque;

/// Epoch-based memory reclamation.
///
/// See [the `crossbeam-epoch` crate](https://github.com/crossbeam-rs/crossbeam-epoch) for more
/// information.
pub mod epoch {
    pub use crossbeam_epoch::*;
}

/// Utilities for concurrent programming.
///
/// See [the `crossbeam-utils` crate](https://github.com/crossbeam-rs/crossbeam-utils) for more
/// information.
pub mod utils {
    pub use crossbeam_utils::*;
}

pub mod sync;

// FIXME(jeehoonkang): this is necessary for re-exporting the `select!` macro, but it also
// introduces a lot of symbols in the global namespace.  We need to find a more precise way to
// re-export `select!`.
pub use crossbeam_channel::*;
