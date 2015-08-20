//! Tools for concurrent and parallel programming.
//!
//! This crate provides a number of building blocks for writing concurrent and parallel code

/*

mem - memory management
    epoch
    cache_padded
sync -
    atomic_option
    msqueue
scope
parallel

*/

#![feature(test)]
#![feature(duration_span)]
#![feature(fnbox)]
#![feature(box_patterns)]
#![feature(box_raw)]
#![feature(const_fn)]
#![feature(optin_builtin_traits)]
#![feature(drain)]

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
