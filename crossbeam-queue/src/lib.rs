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
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

#[cfg_attr(feature = "nightly", cfg(target_has_atomic = "ptr"))]
#[macro_use]
extern crate cfg_if;
#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "std")]
extern crate core;

extern crate crossbeam_utils;

#[cfg_attr(feature = "nightly", cfg(target_has_atomic = "ptr"))]
cfg_if! {
    if #[cfg(feature = "alloc")] {
        mod array_queue;
        mod err;
        mod seg_queue;

        pub use self::array_queue::ArrayQueue;
        pub use self::err::{PopError, PushError};
        pub use self::seg_queue::SegQueue;
    }
}
