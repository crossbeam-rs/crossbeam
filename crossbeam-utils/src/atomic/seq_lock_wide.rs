use core::sync::atomic::{self, AtomicUsize, Ordering};

use Backoff;

/// A simple stamped lock.
///
/// The state is represented as two `AtomicUsize`: `state_hi` for high bits and `state_lo` for low
/// bits.
pub struct SeqLock {
    /// The high bits of the current state of the lock.
    state_hi: AtomicUsize,

    /// The low bits of the current state of the lock.
    ///
    /// All bits except the least significant one hold the current stamp. When locked, the state_lo
    /// equals 1 and doesn't contain a valid stamp.
    state_lo: AtomicUsize,
}

impl SeqLock {
    pub const fn new() -> Self {
        Self {
            state_hi: AtomicUsize::new(0),
            state_lo: AtomicUsize::new(0),
        }
    }

    /// If not locked, returns the current stamp.
    ///
    /// This method should be called before optimistic reads.
    #[inline]
    pub fn optimistic_read(&self) -> Option<(usize, usize)> {
        let state_hi = self.state_hi.load(Ordering::Acquire);
        let state_lo = self.state_lo.load(Ordering::Acquire);
        if state_lo == 1 {
            None
        } else {
            Some((state_hi, state_lo))
        }
    }

    /// Returns `true` if the current stamp is equal to `stamp`.
    ///
    /// This method should be called after optimistic reads to check whether they are valid. The
    /// argument `stamp` should correspond to the one returned by method `optimistic_read`.
    #[inline]
    pub fn validate_read(&self, stamp: (usize, usize)) -> bool {
        atomic::fence(Ordering::Acquire);
        let state_lo = self.state_lo.load(Ordering::Acquire);
        let state_hi = self.state_hi.load(Ordering::Relaxed);
        (state_hi, state_lo) == stamp
    }

    /// Grabs the lock for writing.
    #[inline]
    pub fn write(&'static self) -> SeqLockWriteGuard {
        let backoff = Backoff::new();
        loop {
            let previous = self.state_lo.swap(1, Ordering::Acquire);

            if previous != 1 {
                atomic::fence(Ordering::Release);

                return SeqLockWriteGuard {
                    lock: self,
                    state_lo: previous,
                };
            }

            backoff.snooze();
        }
    }
}

/// A RAII guard that releases the lock and increments the stamp when dropped.
pub struct SeqLockWriteGuard {
    /// The parent lock.
    lock: &'static SeqLock,

    /// The stamp before locking.
    state_lo: usize,
}

impl SeqLockWriteGuard {
    /// Releases the lock without incrementing the stamp.
    #[inline]
    pub fn abort(self) {
        self.lock.state_lo.store(self.state_lo, Ordering::Release);
    }
}

impl Drop for SeqLockWriteGuard {
    #[inline]
    fn drop(&mut self) {
        let state_lo = self.state_lo.wrapping_add(2);

        // Increase the high bits if the low bits wrap around.
        if state_lo == 0 {
            let state_hi = self.lock.state_hi.load(Ordering::Relaxed);
            self.lock.state_hi.store(state_hi.wrapping_add(1), Ordering::Release);
        }

        // Release the lock and increment the stamp.
        self.lock.state_lo.store(state_lo, Ordering::Release);
    }
}
