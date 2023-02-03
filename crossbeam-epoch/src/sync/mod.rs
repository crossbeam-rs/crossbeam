//! Synchronization primitives.

#[cfg(feature = "std")]
#[cfg(not(crossbeam_loom))]
pub(crate) mod once_lock;
