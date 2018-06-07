//! Channel that delivers messages periodically.
//!
//! Messages cannot be sent in this kind of channel; they appear implicitly.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use internal::channel::RecvNonblocking;
use internal::select::{CaseId, Select, Token};

/// Result of a receive operation.
pub type TickToken = Option<Instant>;

/// Channel that delivers messages periodically.
pub struct Channel {
    /// The instant at which the next message will be delivered.
    // TODO: Use `Arc<AtomicCell<Instant>>` here once we implement `AtomicCell`.
    deadline: Arc<Mutex<Instant>>,

    /// The time interval in which messages get delivered.
    duration: Duration,
}

impl Channel {
    /// Creates a channel that delivers messages periodically.
    #[inline]
    pub fn new(dur: Duration) -> Self {
        Channel {
            deadline: Arc::new(Mutex::new(Instant::now() + dur)),
            duration: dur,
        }
    }

    /// Returns a unique identifier for the channel.
    #[inline]
    pub fn channel_id(&self) -> usize {
        self.deadline.as_ref() as *const Mutex<Instant> as usize
    }

    /// Receives a message from the channel.
    #[inline]
    pub fn recv(&self) -> Option<Instant> {
        loop {
            // Compute the time to sleep until the next message.
            let offset = {
                let mut deadline = self.deadline.lock();
                let now = Instant::now();

                // If the deadline has been reached, we can receive the next message.
                if now >= *deadline {
                    let msg = Some(*deadline);
                    *deadline = now + self.duration;
                    return msg;
                }

                *deadline - now
            };

            thread::sleep(offset);
        }
    }

    /// Attempts to receive a message without blocking.
    #[inline]
    pub fn recv_nonblocking(&self) -> RecvNonblocking<Instant> {
        let mut deadline = self.deadline.lock();
        let now = Instant::now();

        // If the deadline has been reached, we can receive the next message.
        if now >= *deadline {
            let msg = RecvNonblocking::Message(*deadline);
            *deadline = now + self.duration;
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
        let deadline = *self.deadline.lock();
        Instant::now() < deadline
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
            deadline: self.deadline.clone(),
            duration: self.duration,
        }
    }
}

impl Select for Channel {
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
        Some(*self.deadline.lock())
    }

    #[inline]
    fn register(&self, _token: &mut Token, _case_id: CaseId) -> bool {
        true
    }

    #[inline]
    fn unregister(&self, _case_id: CaseId) {}

    #[inline]
    fn accept(&self, token: &mut Token) -> bool {
        self.try(token)
    }
}
