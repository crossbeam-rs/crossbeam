//! Concurrent queues.
//!
//! TODO

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

extern crate crossbeam_utils;

mod array_queue;
mod err;
mod seg_queue;

pub use self::array_queue::ArrayQueue;
pub use self::seg_queue::SegQueue;
pub use self::err::{PopError, PushError};
