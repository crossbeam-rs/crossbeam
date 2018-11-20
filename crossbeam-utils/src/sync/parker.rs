use std::fmt;
use std::sync::{Condvar, Mutex};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

const EMPTY: usize = 0;
const PARKED: usize = 1;
const NOTIFIED: usize = 2;

/// A thread parking primitive.
///
/// Only one thread at a time may call [`park`], but any thread may call [`unpark`] at any time.
///
/// Conceptually, each `Parker` has an associated token which is initially not present:
///
/// * The [`park`] method blocks the current thread unless or until the token is available, at
///   which point it automatically consumes the token. It may also return *spuriously*, without
///   consuming the token.
///
/// * The [`park_timeout`] method works the same as [`park`], but blocks for a specified maximum
///   time.
///
/// * The [`unpark`] method atomically makes the token available if it wasn't already. Because the
///   token is initially absent, [`unpark`] followed by [`park`] will result in the second call
///   returning immediately.
///
/// In other words, each `Parker` acts a bit like a spinlock that can be locked and unlocked using
/// [`park`] and [`unpark`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_utils::sync::Parker;
///
/// let parker = Arc::new(Parker::new());
///
/// // Make the token available.
/// parker.unpark();
/// // Wakes up immediately and consumes the token.
/// parker.park();
///
/// let parker2 = parker.clone();
/// thread::spawn(move || {
///     thread::sleep(Duration::from_millis(500));
///     parker2.unpark();
/// });
///
/// // Wakes up when `parker2.unpark()` provides the token, but may also wake up
/// // spuriously before that without consuming the token.
/// parker.park();
/// ```
///
/// [`park`]: struct.Parker.html#method.park
/// [`park_timeout`]: struct.Parker.html#method.park_timeout
/// [`unpark`]: struct.Parker.html#method.unpark
pub struct Parker {
    state: AtomicUsize,
    lock: Mutex<()>,
    cvar: Condvar,
}

impl Parker {
    /// Creates a new `Parker`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::sync::Parker;
    ///
    /// let parker = Parker::new();
    /// ```
    ///
    pub fn new() -> Parker {
        Parker {
            state: AtomicUsize::new(EMPTY),
            lock: Mutex::new(()),
            cvar: Condvar::new(),
        }
    }

    /// Blocks the current thread until the token is made available.
    ///
    /// A call to [`park`] may wake up spuriously without consuming the token, and callers should
    /// be prepared for this possibility.
    ///
    /// Only one thread may call [`park`] or [`park_timeout`] at a time. If multiple threads call
    /// it at the same time, deadlocks or panics might occur.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::sync::Parker;
    ///
    /// let parker = Parker::new();
    ///
    /// // Make the token available.
    /// parker.unpark();
    ///
    /// // Wakes up immediately and consumes the token.
    /// parker.park();
    /// ```
    ///
    /// [`park`]: struct.Parker.html#method.park
    /// [`park_timeout`]: struct.Parker.html#method.park_timeout
    /// [`unpark`]: struct.Parker.html#method.unpark
    pub fn park(&self) {
        self.park_internal(None);
    }

    /// Blocks the current thread until the token is made available, but only for a limited time.
    ///
    /// A call to [`park_timeout`] may wake up spuriously without consuming the token, and callers
    /// should be prepared for this possibility.
    ///
    /// Only one thread may call [`park`] or [`park_timeout`] at a time. If multiple threads call
    /// it at the same time, deadlocks or panics might occur.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use crossbeam_utils::sync::Parker;
    ///
    /// let parker = Parker::new();
    ///
    /// // Waits for the token to become available, but will not wait longer than 500 ms.
    /// parker.park_timeout(Duration::from_millis(500));
    /// ```
    ///
    /// [`park`]: struct.Parker.html#method.park
    /// [`park_timeout`]: struct.Parker.html#method.park_timeout
    /// [`unpark`]: struct.Parker.html#method.unpark
    pub fn park_timeout(&self, timeout: Duration) {
        self.park_internal(Some(timeout));
    }

    fn park_internal(&self, timeout: Option<Duration>) {
        // If we were previously notified then we consume this notification and return quickly.
        if self.state.compare_exchange(NOTIFIED, EMPTY, SeqCst, SeqCst).is_ok() {
            return;
        }

        // If the timeout is zero, then there is no need to actually block.
        if let Some(ref dur) = timeout {
            if *dur == Duration::from_millis(0) {
                return;
            }
        }

        // Otherwise we need to coordinate going to sleep.
        let mut m = self.lock.lock().unwrap();

        match self.state.compare_exchange(EMPTY, PARKED, SeqCst, SeqCst) {
            Ok(_) => {}
            // Consume this notification to avoid spurious wakeups in the next park.
            Err(NOTIFIED) => {
                // We must read `state` here, even though we know it will be `NOTIFIED`. This is
                // because `unpark` may have been called again since we read `NOTIFIED` in the
                // `compare_exchange` above. We must perform an acquire operation that synchronizes
                // with that `unpark` to observe any writes it made before the call to `unpark`. To
                // do that we must read from the write it made to `state`.
                let old = self.state.swap(EMPTY, SeqCst);
                assert_eq!(old, NOTIFIED, "park state changed unexpectedly");
                return;
            }
            Err(n) => panic!("inconsistent park_timeout state: {}", n),
        }

        match timeout {
            None => {
                loop {
                    // Block the current thread on the conditional variable.
                    m = self.cvar.wait(m).unwrap();

                    match self.state.compare_exchange(NOTIFIED, EMPTY, SeqCst, SeqCst) {
                        Ok(_) => return, // got a notification
                        Err(_) => {} // spurious wakeup, go back to sleep
                    }
                }
            }
            Some(timeout) => {
                // Wait with a timeout, and if we spuriously wake up or otherwise wake up from a
                // notification we just want to unconditionally set `state` back to `EMPTY`, either
                // consuming a notification or un-flagging ourselves as parked.
                let (_m, _result) = self.cvar.wait_timeout(m, timeout).unwrap();

                match self.state.swap(EMPTY, SeqCst) {
                    NOTIFIED => {} // got a notification
                    PARKED => {} // no notification
                    n => panic!("inconsistent park_timeout state: {}", n),
                }
            }
        }
    }

    /// Atomically makes the token available if it is not already.
    ///
    /// This method will wake up the thread blocked on [`park`] or [`park_timeout`], if there is
    /// any.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_utils::sync::Parker;
    ///
    /// let parker = Arc::new(Parker::new());
    ///
    /// let parker2 = parker.clone();
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_millis(500));
    ///     parker2.unpark();
    /// });
    ///
    /// // Wakes up when `parker2.unpark()` provides the token, but may also wake up
    /// // spuriously before that without consuming the token.
    /// parker.park();
    /// ```
    ///
    /// [`park`]: struct.Parker.html#method.park
    /// [`park_timeout`]: struct.Parker.html#method.park_timeout
    /// [`unpark`]: struct.Parker.html#method.unpark
    pub fn unpark(&self) {
        // To ensure the unparked thread will observe any writes we made before this call, we must
        // perform a release operation that `park` can synchronize with. To do that we must write
        // `NOTIFIED` even if `state` is already `NOTIFIED`. That is why this must be a swap rather
        // than a compare-and-swap that returns if it reads `NOTIFIED` on failure.
        match self.state.swap(NOTIFIED, SeqCst) {
            EMPTY => return, // no one was waiting
            NOTIFIED => return, // already unparked
            PARKED => {} // gotta go wake someone up
            _ => panic!("inconsistent state in unpark"),
        }

        // There is a period between when the parked thread sets `state` to `PARKED` (or last
        // checked `state` in the case of a spurious wakeup) and when it actually waits on `cvar`.
        // If we were to notify during this period it would be ignored and then when the parked
        // thread went to sleep it would never wake up. Fortunately, it has `lock` locked at this
        // stage so we can acquire `lock` to wait until it is ready to receive the notification.
        //
        // Releasing `lock` before the call to `notify_one` means that when the parked thread wakes
        // it doesn't get woken only to have to wait for us to release `lock`.
        drop(self.lock.lock().unwrap());
        self.cvar.notify_one();
    }
}

impl fmt::Debug for Parker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Parker")
    }
}
