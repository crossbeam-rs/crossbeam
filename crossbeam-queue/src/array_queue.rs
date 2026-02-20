//! The implementation is based on Dmitry Vyukov's bounded MPMC queue.
//!
//! Source:
//!   - <http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue>

#[cfg(not(any(feature = "alloc", feature = "std")))]
mod core;
#[cfg(not(any(feature = "alloc", feature = "std")))]
pub use core::*;

#[cfg(any(feature = "alloc", feature = "std"))]
mod alloc;
#[cfg(any(feature = "alloc", feature = "std"))]
pub use alloc::*;
