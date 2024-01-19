//! Prevent false sharing by padding and aligning to the length of a cache line.

#![no_std]

mod cache_padded;
pub use crate::cache_padded::CachePadded;
