//! Concurrent queues.
//!
//! This crate provides concurrent queues that can be shared among threads:
//!
//! * [`ArrayQueue`], a bounded MPMC queue that allocates a fixed-capacity buffer on construction.
//! * [`SegQueue`], an unbounded MPMC queue that allocates small buffers, segments, on demand.
//!
//! [`ArrayQueue`]: struct.ArrayQueue.html
//! [`SegQueue`]: struct.SegQueue.html

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

extern crate crossbeam_utils;

mod array_queue;
mod err;
mod seg_queue;

pub use self::array_queue::ArrayQueue;
pub use self::seg_queue::SegQueue;
pub use self::err::{PopError, PushError};
