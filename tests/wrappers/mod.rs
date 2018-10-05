//! Wrappers around the channel interface that intentionally enable or disable some optimizations.
//!
//! There's a number of internal optimizations within methods for sending and receiving messages,
//! as well as within the `select!` macro. Such optimizations often diverge from the main code
//! paths to special fast paths with custom implementations.
//!
//! We use these wrappers in order to enable or disable such optimizations and thoroughly exercise
//! the implementation in all possible scenarios.

#![allow(dead_code)]

pub mod cloned;
pub mod normal;
pub mod select;
pub mod select_multi;
pub mod select_spin;

// TODO: add a wrapper with really long send_timeout and recv_timeout
// TODO: select wrappers should do send_timeout, recv_timeout, and try_send
