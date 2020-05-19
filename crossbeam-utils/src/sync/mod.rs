//! Thread synchronization primitives.
//!
//! * [`Parker`], a thread parking primitive.
//! * [`ShardedLock`], a sharded reader-writer lock with fast concurrent reads.
//! * [`WaitGroup`], for synchronizing the beginning or end of some computation.
//!
//! [`Parker`]: struct.Parker.html
//! [`ShardedLock`]: struct.ShardedLock.html
//! [`WaitGroup`]: struct.WaitGroup.html

mod parker;
mod sharded_lock;
mod wait_group;

pub use self::parker::{Parker, Unparker};
pub use self::sharded_lock::{ShardedLock, ShardedLockReadGuard, ShardedLockWriteGuard};
pub use self::wait_group::WaitGroup;

pub use self::parking_lot_sync::*;
mod parking_lot_sync {
// Types that do not need wrapping
 pub use parking_lot::{MutexGuard, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult};
 pub use std::sync::{LockResult, TryLockError, TryLockResult};
 pub use std::sync::Arc;

 use std::time::Duration;

 /// Adapter for `parking_lot::Mutex` to the `std::sync::Mutex` interface.
#[derive(Debug)]
pub struct Mutex<T: ?Sized>(parking_lot::Mutex<T>);

#[derive(Debug)]
pub struct RwLock<T>(parking_lot::RwLock<T>);

/// Adapter for `parking_lot::Condvar` to the `std::sync::Condvar` interface.
#[derive(Debug)]
pub struct Condvar(parking_lot::Condvar);

impl<T> Mutex<T> {
    #[inline]
    pub fn new(t: T) -> Mutex<T> {
        Mutex(parking_lot::Mutex::new(t))
    }

    #[inline]
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        Ok(self.0.lock())
    }

    #[inline]
    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        match self.0.try_lock() {
            Some(guard) => Ok(guard),
            None => Err(TryLockError::WouldBlock),
        }
    }
}

impl<T: ?Sized + Default> Default for Mutex<T> {
    /// Creates a `Mutex<T>`, with the `Default` value for T.
    fn default() -> Mutex<T> {
        Mutex::new(Default::default())
    }
}


impl<T> RwLock<T> {
    pub fn new(t: T) -> RwLock<T> {
        RwLock(parking_lot::RwLock::new(t))
    }

    pub fn read(&self) -> LockResult<RwLockReadGuard<'_, T>> {
        Ok(self.0.read())
    }

    pub fn write(&self) -> LockResult<RwLockWriteGuard<'_, T>> {
        Ok(self.0.write())
    }
}

impl Condvar {
    #[inline]
    pub fn new() -> Condvar {
        Condvar(parking_lot::Condvar::new())
    }

    #[inline]
    pub fn notify_one(&self) {
        self.0.notify_one();
    }

    #[inline]
    pub fn notify_all(&self) {
        self.0.notify_all();
    }

    #[inline]
    pub fn wait<'a, T>(
        &self,
        mut guard: MutexGuard<'a, T>,
    ) -> LockResult<MutexGuard<'a, T>> {
        self.0.wait(&mut guard);
        Ok(guard)
    }

    #[inline]
    pub fn wait_timeout<'a, T>(
        &self,
        mut guard: MutexGuard<'a, T>,
        timeout: Duration,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let wtr = self.0.wait_for(&mut guard, timeout);
        Ok((guard, wtr))
    }
}

}
