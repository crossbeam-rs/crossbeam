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
//! primitives (locks, semaphores, barriers, etc) but this crate seeks to expand
//! that set to include more advanced/niche primitives, as well as userspace
//! alternatives. These live in the `sync` module.
//!
//! - **Scoped thread API**. Finally, the crate provides a "scoped" thread API,
//! making it possible to spawn threads that share stack data with their
//! parents. This functionality is exported at the top-level.

//#![deny(missing_docs)]

#![cfg_attr(feature = "nightly",
            feature(const_fn, repr_simd, optin_builtin_traits))]

use std::thread;

pub use scoped::{scope, Scope, ScopedJoinHandle};

pub mod mem;
pub mod sync;
mod scoped;

#[doc(hidden)]
trait FnBox {
    fn call_box(self: Box<Self>);
}

impl<F: FnOnce()> FnBox for F {
    fn call_box(self: Box<Self>) { (*self)() }
}

/// Like `std::thread::spawn`, but without the closure bounds.
pub unsafe fn spawn_unsafe<'a, F>(f: F) -> thread::JoinHandle<()> where F: FnOnce() + Send + 'a {
    use std::mem;

    let closure: Box<FnBox + 'a> = Box::new(f);
    let closure: Box<FnBox + Send> = mem::transmute(closure);
    thread::spawn(move || closure.call_box())
}
