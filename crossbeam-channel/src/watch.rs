//! A watch channel that only stores the latest value.
//!
//! Sending never blocks (it overwrites the latest value), while receiving
//! blocks until a new value is available.

use alloc::sync::Arc;
use core::fmt;
use std::sync::{Condvar, Mutex};

use crate::err::{RecvError, SendError, TryRecvError};

struct Inner<T> {
    /// The latest value, if any.
    value: Mutex<State<T>>,
    /// Condvar to notify receivers when a new value is available.
    notify: Condvar,
}

struct State<T> {
    /// The stored value, if any.
    value: Option<T>,
    /// Incremented each time a new value is sent.
    version: u64,
    /// Whether all senders have been dropped.
    disconnected: bool,
    /// Number of active senders.
    sender_count: usize,
    /// Number of active receivers.
    receiver_count: usize,
}

/// The sending side of a watch channel.
///
/// Senders can be cloned. When all senders are dropped, the channel becomes
/// disconnected.
pub struct WatchSender<T> {
    inner: Arc<Inner<T>>,
}

/// The receiving side of a watch channel.
///
/// Receivers can be cloned. Each receiver independently tracks which version
/// it has seen, so each receiver will get the next new value sent after its
/// last receive.
pub struct WatchReceiver<T> {
    inner: Arc<Inner<T>>,
    /// The version this receiver last saw.
    seen_version: u64,
}

/// Creates a watch channel.
///
/// The channel stores only the latest sent value. Sending never blocks; it
/// simply overwrites the current value. Receiving blocks until a new value
/// is available (i.e., one with a version newer than what the receiver last
/// saw).
///
/// Returns a [`WatchSender`] and [`WatchReceiver`] pair.
///
/// # Examples
///
/// ```
/// use crossbeam_channel::watch;
///
/// let (tx, mut rx) = watch::<i32>();
///
/// tx.send(1).unwrap();
/// tx.send(2).unwrap();
///
/// // Only the latest value is available.
/// assert_eq!(rx.recv(), Ok(2));
///
/// drop(tx);
/// assert_eq!(rx.recv(), Err(crossbeam_channel::RecvError));
/// ```
pub fn watch<T>() -> (WatchSender<T>, WatchReceiver<T>) {
    let inner = Arc::new(Inner {
        value: Mutex::new(State {
            value: None,
            version: 0,
            disconnected: false,
            sender_count: 1,
            receiver_count: 1,
        }),
        notify: Condvar::new(),
    });

    let tx = WatchSender {
        inner: Arc::clone(&inner),
    };
    let rx = WatchReceiver {
        inner,
        seen_version: 0,
    };
    (tx, rx)
}

impl<T> WatchSender<T> {
    /// Sends a value into the channel, overwriting any previously stored value.
    ///
    /// This operation never blocks.
    ///
    /// Returns an error if all receivers have been dropped.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut state = self.inner.value.lock().unwrap();
        if state.receiver_count == 0 {
            return Err(SendError(value));
        }
        state.value = Some(value);
        state.version += 1;
        // Notify all waiting receivers.
        self.inner.notify.notify_all();
        Ok(())
    }
}

impl<T> Clone for WatchSender<T> {
    fn clone(&self) -> Self {
        let mut state = self.inner.value.lock().unwrap();
        state.sender_count += 1;
        WatchSender {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Drop for WatchSender<T> {
    fn drop(&mut self) {
        let mut state = self.inner.value.lock().unwrap();
        state.sender_count -= 1;
        if state.sender_count == 0 {
            state.disconnected = true;
            self.inner.notify.notify_all();
        }
    }
}

impl<T> fmt::Debug for WatchSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("WatchSender { .. }")
    }
}

impl<T> WatchReceiver<T> {
    /// Blocks until a new value is available and returns it.
    ///
    /// Returns an error if all senders have been dropped and no new value
    /// is available.
    pub fn recv(&mut self) -> Result<T, RecvError>
    where
        T: Clone,
    {
        let mut state = self.inner.value.lock().unwrap();
        loop {
            // Check if there's a value with a newer version than what we've seen.
            if state.version > self.seen_version {
                if let Some(ref value) = state.value {
                    self.seen_version = state.version;
                    return Ok(value.clone());
                }
            }
            if state.disconnected {
                return Err(RecvError);
            }
            state = self.inner.notify.wait(state).unwrap();
        }
    }

    /// Attempts to receive the latest value without blocking.
    ///
    /// Returns an error if no new value is available or the channel is
    /// disconnected.
    pub fn try_recv(&mut self) -> Result<T, TryRecvError>
    where
        T: Clone,
    {
        let state = self.inner.value.lock().unwrap();
        if state.version > self.seen_version {
            if let Some(ref value) = state.value {
                self.seen_version = state.version;
                return Ok(value.clone());
            }
        }
        if state.disconnected {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
}

impl<T> Clone for WatchReceiver<T> {
    fn clone(&self) -> Self {
        let mut state = self.inner.value.lock().unwrap();
        state.receiver_count += 1;
        WatchReceiver {
            inner: Arc::clone(&self.inner),
            seen_version: self.seen_version,
        }
    }
}

impl<T> Drop for WatchReceiver<T> {
    fn drop(&mut self) {
        let mut state = self.inner.value.lock().unwrap();
        state.receiver_count -= 1;
        // If no receivers left, notify senders so they get errors.
    }
}

impl<T> fmt::Debug for WatchReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("WatchReceiver { .. }")
    }
}
