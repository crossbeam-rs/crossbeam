//! Memory management for concurrent data structures
//!
//! At the moment, the only memory management scheme is epoch-based reclamation,
//! found in the `epoch` submodule.

pub use self::cache_padded::{CachePadded, ZerosValid};

pub mod epoch;
mod cache_padded;
