//! Channel that delivers messages periodically.
//!
//! Messages cannot be sent into this kind of channel; they are materialized on demand.

use std::num::Wrapping;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use err::{RecvTimeoutError, TryRecvError};
use internal::context::Context;
use internal::select::{Operation, SelectHandle, Token};

// TODO: rename deadline to something better

/// Result of a receive operation.
pub type TickToken = Option<Instant>;

/// Channel state.
struct Inner {
    /// The instant at which the next message will be delivered.
    deadline: Instant,

    /// The index of the next message to be received.
    index: Wrapping<usize>,
}

/// Channel that delivers messages periodically.
pub struct Channel {
    /// The state of the channel.
    // TODO: Use `Arc<AtomicCell<Inner>>` here once we implement `AtomicCell`.
    inner: Arc<Mutex<Inner>>,

    /// The time interval in which messages get delivered.
    duration: Duration,
}

impl Channel {
    /// Creates a channel that delivers messages periodically.
    #[inline]
    pub fn new(dur: Duration) -> Self {
        Channel {
            inner: Arc::new(Mutex::new(Inner {
                deadline: Instant::now() + dur,
                index: Wrapping(0),
            })),
            duration: dur,
        }
    }

    /// Attempts to receive a message without blocking.
    #[inline]
    pub fn try_recv(&self) -> Result<Instant, TryRecvError> {
        let mut inner = self.inner.lock();
        let now = Instant::now();

        // If the deadline has been reached, we can receive the next message.
        if now >= inner.deadline {
            let msg = inner.deadline;
            inner.deadline = now + self.duration;
            inner.index += Wrapping(1);
            Ok(msg)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    /// Receives a message from the channel.
    #[inline]
    pub fn recv(&self, deadline: Option<Instant>) -> Result<Instant, RecvTimeoutError> {
        loop {
            // Compute the time to sleep until the next message or the deadline.
            let offset = {
                let mut inner = self.inner.lock();
                let now = Instant::now();

                // Check if we can receive the next message.
                if now >= inner.deadline {
                    let msg = inner.deadline;
                    inner.deadline = now + self.duration;
                    inner.index += Wrapping(1);
                    return Ok(msg);
                }

                // Check if the operation deadline has been reached.
                if let Some(d) = deadline {
                    if now >= d {
                        return Err(RecvTimeoutError::Timeout);
                    }

                    inner.deadline.min(d) - now
                } else {
                    inner.deadline - now
                }
            };

            thread::sleep(offset);
        }
    }

    /// Reads a message from the channel.
    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Result<Instant, ()> {
        token.tick.ok_or(())
    }

    /// Returns `true` if the channel is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        let inner = self.inner.lock();
        Instant::now() < inner.deadline
    }

    /// Returns the number of messages in the channel.
    #[inline]
    pub fn len(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            1
        }
    }

    /// Returns the capacity of the channel.
    #[inline]
    pub fn capacity(&self) -> Option<usize> {
        Some(1)
    }
}

impl Clone for Channel {
    #[inline]
    fn clone(&self) -> Channel {
        Channel {
            inner: self.inner.clone(),
            duration: self.duration,
        }
    }
}

impl SelectHandle for Channel {
    #[inline]
    fn try(&self, token: &mut Token) -> bool {
        match self.try_recv() {
            Ok(msg) => {
                token.tick = Some(msg);
                true
            }
            Err(TryRecvError::Disconnected) => {
                token.tick = None;
                true
            }
            Err(TryRecvError::Empty) => {
                false
            }
        }
    }

    #[inline]
    fn retry(&self, token: &mut Token) -> bool {
        self.try(token)
    }

    #[inline]
    fn deadline(&self) -> Option<Instant> {
        Some(self.inner.lock().deadline)
    }

    #[inline]
    fn register(&self, _token: &mut Token, _oper: Operation, _cx: &Context) -> bool {
        true
    }

    #[inline]
    fn unregister(&self, _oper: Operation) {}

    #[inline]
    fn accept(&self, token: &mut Token, _cx: &Context) -> bool {
        self.try(token)
    }

    #[inline]
    fn state(&self) -> usize {
        // Return the index of the next message to be delivered to the channel.
        let inner = self.inner.lock();
        let index = if Instant::now() < inner.deadline {
            inner.index
        } else {
            inner.index + Wrapping(1)
        };
        index.0
    }
}
