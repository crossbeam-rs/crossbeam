//! Thread synchronization primitives.
//!
//! * [`Parker`], a thread parking primitive.
//! * [`ShardedLock`], a sharded reader-writer lock with fast concurrent reads.
//! * [`WaitGroup`], for synchronizing the beginning or end of some computation.

mod parker;
#[cfg(not(loom))]
mod sharded_lock;
mod wait_group;

pub use self::parker::{Parker, Unparker};
#[cfg(not(loom))]
pub use self::sharded_lock::{ShardedLock, ShardedLockReadGuard, ShardedLockWriteGuard};
pub use self::wait_group::WaitGroup;
