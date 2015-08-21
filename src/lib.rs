//! Tools for concurrent and parallel programming.
//!
//! This crate is an early work in progress. The focus for the moment is
//! concurrency:
//!
//! - **Non-blocking data structures**. These data structures allow for high
//! performance, highly-concurrent access, much superior to wrapping with a
//! `Mutex`. Ultimately the goal is to include stacks, queues, deques, bags,
//! sets and maps.
//!
//! - **Memory management**. Because non-blocking data structures avoid global
//! synchronization, it is not easy to tell when internal data can be safely
//! freed. The `mem` module provides generic, easy to use, and high-performance
//! APIs for managing memory in these cases.
//!
//! - **Synchronization**. The standard library provides a few synchronization
//! primitives (locks, semaphores, barriers, etc) but this crate seeks to expand
//! that set to include more advanced/niche primitives, as well as userspace
//! alternatives.
//!
//! - **Scoped thread API**. Finally, the crate provides a "scoped" thread API,
//! making it possible to spawn threads that share stack data with their
//! parents.

#![feature(fnbox)]
#![feature(box_patterns)]
#![feature(box_raw)]
#![feature(const_fn)]
#![feature(optin_builtin_traits)]

use std::boxed::FnBox;
use std::thread;

pub use scoped::{scope, Scope, ScopedJoinHandle};

pub mod mem;
pub mod sync;
mod scoped;

pub unsafe fn spawn_unsafe<'a, F>(f: F) -> thread::JoinHandle<()> where F: FnOnce() + 'a {
    use std::mem;

    let closure: Box<FnBox() + 'a> = Box::new(f);
    let closure: Box<FnBox() + Send> = mem::transmute(closure);
    thread::spawn(closure)
}
