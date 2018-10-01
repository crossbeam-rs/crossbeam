//! A scalable reader-writer lock.
//!
//! This implementation makes read operations faster and more scalable due to less contention,
//! while making write operations slower. It also incurs much higher memory overhead than
//! traditional reader-writer locks.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ops::{Deref, DerefMut};

use num_cpus;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use thread;

use CachePadded;

/// A scalable reader-writer lock.
///
/// This type of lock allows a number of readers or at most one writer at any point in time. The
/// write portion of this lock typically allows modification of the underlying data (exclusive
/// access) and the read portion of this lock typically allows for read-only access (shared
/// access).
///
/// This reader-writer lock differs from typical implementations in that it internally creates a
/// list of reader-writer locks called 'shards'. Shards are aligned and padded to the cache line
/// size.
///
/// Read operations lock only one shard specific to the current thread, while write operations lock
/// every shard in succession. This strategy makes concurrent read operations faster due to less
/// contention, but write operations are slower due to increased amount of locking.
pub struct ShardedLock<T> {
    /// A list of locks protecting the internal data.
    shards: Vec<CachePadded<RwLock<()>>>,

    /// The internal data.
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for ShardedLock<T> {}
unsafe impl<T: Send + Sync> Sync for ShardedLock<T> {}

impl<T> ShardedLock<T> {
    /// Creates a new `ShardedLock` initialized with `value`.
    pub fn new(value: T) -> ShardedLock<T> {
        // The number of shards is a power of two so that the modulo operation in `read` becomes a
        // simple bitwise "and".
        let num_shards = num_cpus::get().next_power_of_two();

        ShardedLock {
            shards: (0..num_shards)
                .map(|_| CachePadded::new(RwLock::new(())))
                .collect(),
            value: UnsafeCell::new(value),
        }
    }

    /// Locks with shared read access, blocking the current thread until it can be acquired.
    ///
    /// The calling thread will be blocked until there are no more writers which hold the lock.
    /// There may be other readers currently inside the lock when this method returns. This method
    /// does not provide any guarantees with respect to the ordering of whether contentious readers
    /// or writers will acquire the lock first.
    ///
    /// Returns an RAII guard which will drop the read access of this lock when dropped.
    pub fn read(&self) -> ShardedLockReadGuard<T> {
        // Take the current thread index and map it to a shard index. Thread indices will tend to
        // distribute shards among threads equally, thus reducing contention due to read-locking.
        let current_index = thread::current_index().unwrap_or(0);
        let shard_index = current_index & (self.shards.len() - 1);

        ShardedLockReadGuard {
            parent: self,
            _guard: self.shards[shard_index].read(),
            _marker: PhantomData,
        }
    }

    /// Locks with exclusive write access, blocking the current thread until it can be acquired.
    ///
    /// This function will not return while other writers or other readers currently have access to
    /// the lock.
    ///
    /// Returns an RAII guard which will drop the write access of this lock when dropped.
    pub fn write(&self) -> ShardedLockWriteGuard<T> {
        // Write-lock each shard in succession.
        for shard in &self.shards {
            // The write guard is forgotten, but the lock will be manually unlocked in `drop`.
            mem::forget(shard.write());
        }

        ShardedLockWriteGuard {
            parent: self,
            _marker: PhantomData,
        }
    }
}

/// A guard used to release the shared read access of a `ShardedLock` when dropped.
pub struct ShardedLockReadGuard<'a, T: 'a> {
    parent: &'a ShardedLock<T>,
    _guard: RwLockReadGuard<'a, ()>,
    _marker: PhantomData<RwLockReadGuard<'a, T>>,
}

unsafe impl<'a, T: Sync> Sync for ShardedLockReadGuard<'a, T> {}

impl<'a, T> Deref for ShardedLockReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.parent.value.get() }
    }
}

/// A guard used to release the exclusive write access of a `ShardedLock` when dropped.
pub struct ShardedLockWriteGuard<'a, T: 'a> {
    parent: &'a ShardedLock<T>,
    _marker: PhantomData<RwLockWriteGuard<'a, T>>,
}

unsafe impl<'a, T: Sync> Sync for ShardedLockWriteGuard<'a, T> {}

impl<'a, T> Drop for ShardedLockWriteGuard<'a, T> {
    fn drop(&mut self) {
        // Unlock the shards in reverse order of locking.
        for shard in self.parent.shards.iter().rev() {
            unsafe {
                (**shard).force_unlock_write();
            }
        }
    }
}

impl<'a, T> Deref for ShardedLockWriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.parent.value.get() }
    }
}

impl<'a, T> DerefMut for ShardedLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.parent.value.get() }
    }
}
