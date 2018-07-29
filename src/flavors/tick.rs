//! Channel that delivers messages periodically.
//!
//! Messages cannot be sent into this kind of channel; they are materialized on demand.

use std::num::Wrapping;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use internal::channel::RecvNonblocking;
use internal::select::{Operation, SelectHandle, Token};

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

    /// Returns a unique identifier for the channel.
    #[inline]
    pub fn channel_id(&self) -> usize {
        self.inner.as_ref() as *const Mutex<Inner> as usize
    }

    /// Receives a message from the channel.
    #[inline]
    pub fn recv(&self) -> Option<Instant> {
        loop {
            // Compute the time to sleep until the next message.
            let offset = {
                let mut inner = self.inner.lock();
                let now = Instant::now();

                // If the deadline has been reached, we can receive the next message.
                if now >= inner.deadline {
                    let msg = Some(inner.deadline);
                    inner.deadline = now + self.duration;
                    inner.index += Wrapping(1);
                    return msg;
                }

                inner.deadline - now
            };

            thread::sleep(offset);
        }
    }

    /// Attempts to receive a message without blocking.
    #[inline]
    pub fn recv_nonblocking(&self) -> RecvNonblocking<Instant> {
        let mut inner = self.inner.lock();
        let now = Instant::now();

        // If the deadline has been reached, we can receive the next message.
        if now >= inner.deadline {
            let msg = RecvNonblocking::Message(inner.deadline);
            inner.deadline = now + self.duration;
            inner.index += Wrapping(1);
            msg
        } else {
            RecvNonblocking::Empty
        }
    }

    /// Reads a message from the channel.
    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Option<Instant> {
        token.tick
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
        match self.recv_nonblocking() {
            RecvNonblocking::Message(msg) => {
                token.tick = Some(msg);
                true
            }
            RecvNonblocking::Closed => {
                token.tick = None;
                true
            }
            RecvNonblocking::Empty => {
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
    fn register(&self, _token: &mut Token, _oper: Operation) -> bool {
        true
    }

    #[inline]
    fn unregister(&self, _oper: Operation) {}

    #[inline]
    fn accept(&self, token: &mut Token) -> bool {
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
