//! Concurrent stacks.
//!
//! This crate provides concurrent stacks that can be shared among threads:
//!
//! * [`ArrayStack`], a bounded stack that allocates a fixed-capacity buffer on construction.
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
mod array_stack;

#[cfg(all(feature = "alloc", target_has_atomic = "ptr"))]
pub use crate::array_stack::ArrayStack;
