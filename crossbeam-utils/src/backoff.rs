use core::cell::Cell;
use core::fmt;
use core::sync::atomic;

const SPIN_LIMIT: u32 = 6;
const YIELD_LIMIT: u32 = 10;

/// A counter for exponential backoff in spin loops.
///
/// Exponential backoff reduces contention and improves performance in spin loops.
///
/// # Examples
///
/// TODO a classic sketch with spin, snooze, block
///
/// ```
/// use crossbeam_utils::Backoff;
/// use std::sync::atomic::{AtomicBool, AtomicUsize};
/// use std::sync::atomic::Ordering::SeqCst;
///
/// fn fetch_mul(a: &AtomicUsize, b: usize) -> usize {
///     let backoff = Backoff::new();
///     loop {
///         let val = a.load(SeqCst);
///         if a.compare_and_swap(val, val.wrapping_mul(b), SeqCst) == val {
///             return val;
///         }
///         backoff.spin();
///     }
/// }
///
/// fn spin_wait(ready: &AtomicBool) {
///     let backoff = Backoff::new();
///     while !ready.load(SeqCst) {
///         backoff.snooze();
///     }
/// }
///
/// fn blocking_wait(ready: &AtomicBool) {
///     let backoff = Backoff::new();
///     while !ready.load(SeqCst) {
///         if backoff.is_complete() {
///             thread::park();
///         } else {
///             backoff.snooze();
///         }
///     }
/// }
/// ```
pub struct Backoff {
    step: Cell<u32>,
}

impl Backoff {
    /// Creates a new `Backoff`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::Backoff;
    ///
    /// let backoff = Backoff::new();
    /// ```
    #[inline]
    pub fn new() -> Self {
        Backoff {
            step: Cell::new(0),
        }
    }

    /// Resets the `Backoff`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::Backoff;
    ///
    /// let backoff = Backoff::new();
    /// backoff.reset();
    /// ```
    #[inline]
    pub fn reset(&self) {
        self.step.set(0);
    }

    /// Backs off in a lock-free loop.
    ///
    /// This method should be used when we need to retry an operation because another thread made
    /// progress.
    ///
    /// The processor may yield using the "YIELD" or "PAUSE" instruction.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::Backoff;
    /// use std::sync::atomic::AtomicUsize;
    /// use std::sync::atomic::Ordering::SeqCst;
    ///
    /// fn fetch_mul(a: &AtomicUsize, b: usize) -> usize {
    ///     let backoff = Backoff::new();
    ///     loop {
    ///         let val = a.load(SeqCst);
    ///         if a.compare_and_swap(val, val.wrapping_mul(b), SeqCst) == val {
    ///             return val;
    ///         }
    ///         backoff.spin();
    ///     }
    /// }
    ///
    /// let a = AtomicUsize::new(7);
    /// assert_eq!(fetch_mul(&a, 8), 7);
    /// assert_eq!(a.load(SeqCst), 56);
    /// ```
    #[inline]
    pub fn spin(&self) {
        for _ in 0..1 << self.step.get().min(SPIN_LIMIT) {
            atomic::spin_loop_hint();
        }

        if self.step.get() <= SPIN_LIMIT {
            self.step.set(self.step.get() + 1);
        }
    }

    /// Backs off in a blocking loop.
    ///
    /// This method should be used when we need to wait for another thread to make progress.
    ///
    /// The processor may yield using the "YIELD" or "PAUSE" instruction and the current thread
    /// may yield by giving up a timeslice to the OS scheduler.
    ///
    /// In `#[no_std]` environments, this method is equivalent to [`spin`].
    ///
    /// If possible, use [`is_complete`] to check when it is advised to stop using backoff and
    /// block the current thread using a different synchronization mechanism instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::Backoff;
    /// use std::sync::Arc;
    /// use std::sync::atomic::AtomicBool;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use std::thread;
    /// use std::time::Duration;
    ///
    /// fn wait(ready: &AtomicBool) {
    ///     let backoff = Backoff::new();
    ///     while !ready.load(SeqCst) {
    ///         backoff.snooze();
    ///     }
    /// }
    ///
    /// let ready = Arc::new(AtomicBool::new(false));
    /// let ready2 = ready.clone();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_millis(100));
    ///     ready2.store(true, SeqCst);
    /// });
    ///
    /// assert_eq!(ready.load(SeqCst), false);
    /// wait(&ready);
    /// assert_eq!(ready.load(SeqCst), true);
    /// ```
    #[inline]
    pub fn snooze(&self) {
        if self.step.get() <= SPIN_LIMIT {
            for _ in 0..1 << self.step.get() {
                atomic::spin_loop_hint();
            }
        } else {
            #[cfg(not(feature = "std"))]
            for _ in 0..1 << self.step.get() {
                atomic::spin_loop_hint();
            }

            #[cfg(feature = "std")]
            ::std::thread::yield_now();
        }

        if self.step.get() <= YIELD_LIMIT {
            self.step.set(self.step.get() + 1);
        }
    }

    /// Returns `true` if backoff is complete and blocking the thread is advised.
    ///
    /// TODO
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::Backoff;
    /// use std::sync::Arc;
    /// use std::sync::atomic::AtomicBool;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use std::thread;
    /// use std::time::Duration;
    ///
    /// fn wait(ready: &AtomicBool) {
    ///     let backoff = Backoff::new();
    ///     while !ready.load(SeqCst) {
    ///         if backoff.is_complete() {
    ///             thread::park();
    ///         } else {
    ///             backoff.snooze();
    ///         }
    ///     }
    /// }
    ///
    /// let ready = Arc::new(AtomicBool::new(false));
    /// let ready2 = ready.clone();
    /// let waiter = thread::current();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_millis(100));
    ///     ready2.store(true, SeqCst);
    ///     waiter.unpark();
    /// });
    ///
    /// assert_eq!(ready.load(SeqCst), false);
    /// wait(&ready);
    /// assert_eq!(ready.load(SeqCst), true);
    /// ```
    #[inline]
    pub fn is_complete(&self) -> bool {
        self.step.get() > YIELD_LIMIT
    }
}

impl fmt::Debug for Backoff {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Backoff")
            .field("step", &self.step)
            .field("is_complete", &self.is_complete())
            .finish()
    }
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff::new()
    }
}
