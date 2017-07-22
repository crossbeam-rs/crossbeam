use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::time::{Duration, Instant};

use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use monitor::Monitor;

pub mod array;
pub mod list;
pub mod zero;

pub trait Channel<T> {
    fn try_send(&self, value: T) -> Result<(), TrySendError<T>>;
    fn send_until(&self, value: T, deadline: Option<Instant>) -> Result<(), SendTimeoutError<T>>;

    fn try_recv(&self) -> Result<T, TryRecvError>;
    fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError>;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool;
    fn is_full(&self) -> bool;
    fn capacity(&self) -> Option<usize>;

    fn close(&self) -> bool;
    fn is_closed(&self) -> bool;

    fn monitor(&self) -> &Monitor;
    fn is_ready(&self) -> bool;

    fn send(&self, value: T) -> Result<(), SendError<T>> {
        match self.send_until(value, None) {
            Ok(()) => Ok(()),
            Err(SendTimeoutError::Disconnected(v)) => Err(SendError(v)),
            Err(SendTimeoutError::Timeout(v)) => Err(SendError(v)),
        }
    }

    fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        self.send_until(value, Some(Instant::now() + dur))
    }

    fn recv(&self) -> Result<T, RecvError> {
        if let Ok(v) = self.recv_until(None) {
            Ok(v)
        } else {
            Err(RecvError)
        }
    }

    fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        self.recv_until(Some(Instant::now() + dur))
    }
}
