use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::time::{Duration, Instant};

use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};

pub mod array;
pub mod list;
pub mod zero;

pub trait Channel<T> {
    fn try_send(&self, value: T) -> Result<(), TrySendError<T>>;

    fn send_until(&self, value: T, deadline: Option<Instant>) -> Result<(), SendTimeoutError<T>>;

    fn try_recv(&self) -> Result<T, TryRecvError>;

    fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError>;

    fn len(&self) -> usize;

    /// Returns `true` if receiving would block.
    fn is_empty(&self) -> bool;

    /// Returns `true` if sending would block.
    fn is_full(&self) -> bool;

    fn capacity(&self) -> Option<usize>;

    fn close(&self) -> bool;
    fn is_closed(&self) -> bool;

    fn id(&self) -> usize {
        self as *const Self as *const u8 as usize
    }
}
