//! Additional utilities for atomics.

mod atomic_cell;
mod consume;

pub use self::atomic_cell::AtomicCell;
pub use self::consume::AtomicConsume;
