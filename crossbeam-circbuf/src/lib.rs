//! Concurrent queues based on circular buffer.
//!
//! Currently, this crate provides the following flavors of queues:
//!
//! - bounded/unbounded SPSC (single-producer single-consumer)
//! - bounded/unbounded SPMC (single-producer multiple-consumer)
//! - bounded MPMC (multiple-producer multiple-consumer)

#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]

extern crate core;
extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;

mod buffer;
mod try_recv;

#[doc(hidden)] // for doc-tests
pub mod sp_inner;

pub use try_recv::TryRecv;

pub mod mpmc;
pub mod spmc;
pub mod spsc;
