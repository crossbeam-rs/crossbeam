//! Hidden public internals of the crate.
//!
//! The only reason why this module is public is because it must be accessible from the `select!`
//! macro.

pub extern crate smallvec;

#[macro_use]
pub mod select;

pub mod channel;
pub mod context;
pub mod utils;
pub mod waker;
