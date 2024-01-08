//! Concurrent queues.
//!
//! This crate provides concurrent queues that can be shared among threads:
//!
//! * [`ArrayQueue`], a bounded MPMC queue that allocates a fixed-capacity buffer on construction.
//! * [`SegQueue`], an unbounded MPMC queue that allocates small buffers, segments, on demand.

#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms),
        allow(dead_code, unused_assignments, unused_variables)
    )
))]
#![warn(
    missing_docs,
    missing_debug_implementations,
    rust_2018_idioms,
    unreachable_pub
)]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
extern crate alloc;

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
mod array_queue;
#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
mod seg_queue;

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
pub use crate::{array_queue::ArrayQueue, seg_queue::SegQueue};
