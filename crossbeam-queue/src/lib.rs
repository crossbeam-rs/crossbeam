//! Concurrent queues.
//!
//! This crate provides concurrent queues that can be shared among threads:
//!
//! * [`ArrayQueue`], a bounded MPMC queue that allocates a fixed-capacity buffer on construction.
//! * [`SegQueue`], an unbounded MPMC queue that allocates small buffers, segments, on demand.

#![no_std]
#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms, single_use_lifetimes),
        allow(dead_code, unused_assignments, unused_variables)
    )
))]
#![warn(missing_docs, unsafe_op_in_unsafe_fn)]

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
mod array_queue;
#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
mod seg_queue;

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
pub use crate::{array_queue::ArrayQueue, seg_queue::SegQueue};
