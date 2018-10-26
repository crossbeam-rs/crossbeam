//! Synchronization primitives.

mod sharded_lock;
mod wait_group;

pub use self::sharded_lock::{ShardedLock, ShardedLockReadGuard, ShardedLockWriteGuard};
pub use self::wait_group::WaitGroup;
