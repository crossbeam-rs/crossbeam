//! Support for concurrent and parallel programming.
//!
//! This crate is an early work in progress. The focus for the moment is
//! concurrency:
//!
//! - **Non-blocking data structures**. These data structures allow for high
//! performance, highly-concurrent access, much superior to wrapping with a
//! `Mutex`. Ultimately the goal is to include stacks, queues, deques, bags,
//! sets and maps. These live in the `sync` module.
//!
//! - **Memory management**. Because non-blocking data structures avoid global
//! synchronization, it is not easy to tell when internal data can be safely
//! freed. The `mem` module provides generic, easy to use, and high-performance
//! APIs for managing memory in these cases. These live in the `mem` module.
//!
//! - **Synchronization**. The standard library provides a few synchronization
//! primitives (locks, barriers, etc) but this crate seeks to expand that set
//! to include more advanced/niche primitives, as well as userspace
//! alternatives. These live in the `sync` module.
//!
//! - **Scoped thread API**. Finally, the crate provides a "scoped" thread API,
//! making it possible to spawn threads that share stack data with their
//! parents. This functionality is exported at the top-level.

//#![deny(missing_docs)]

pub extern crate crossbeam_epoch as epoch;
pub extern crate crossbeam_utils as utils;

pub mod sync;
pub use utils::cache_padded::CachePadded;
pub use utils::scoped::{scope, Scope, ScopedJoinHandle, ScopedThreadBuilder};
