//! Synchronization primitives.

mod sharded_lock;

pub use self::sharded_lock::{ShardedLock, ShardedLockReadGuard, ShardedLockWriteGuard};
