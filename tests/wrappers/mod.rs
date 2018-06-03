//! Wrappers around the channel interface that turn on and off various optimizations.
//!
//! There's a number of internal optimizations within methods for sending and receiving messages,
//! as well as within the `select!` macro. Such optimizations often diverge from the main code
//! paths to special fast paths with custom implementations.
//!
//! We use these wrappers in order to turn on and off such optimizations and thoroughly exercise
//! the tests in all possible configurations.

#![allow(dead_code)]

pub mod cloned;
pub mod normal;
pub mod select;
pub mod select_multi;
pub mod select_spin;
