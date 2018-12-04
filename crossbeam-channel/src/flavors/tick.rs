//! Channel that delivers messages periodically.
//!
//! Messages cannot be sent into this kind of channel; they are materialized on demand.

use std::num::Wrapping;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use context::Context;
use err::{RecvTimeoutError, TryRecvError};
use select::{Operation, SelectHandle, Token};

/// Result of a receive operation.
pub type TickToken = Option<Instant>;

/// Channel state.
struct Inner {
    /// The instant at which the next message will be delivered.
    next_tick: Instant,

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
                next_tick: Instant::now() + dur,
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

        // If the next tick time has been reached, we can receive the next message.
        if now >= inner.next_tick {
            let msg = inner.next_tick;
            inner.next_tick = now + self.duration;
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
                if now >= inner.next_tick {
                    let msg = inner.next_tick;
                    inner.next_tick = now + self.duration;
                    inner.index += Wrapping(1);
                    return Ok(msg);
                }

                // Check if the operation deadline has been reached.
                if let Some(d) = deadline {
                    if now >= d {
                        return Err(RecvTimeoutError::Timeout);
                    }

                    inner.next_tick.min(d) - now
                } else {
                    inner.next_tick - now
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
        Instant::now() < inner.next_tick
    }

    /// Returns `true` if the channel is full.
    #[inline]
    pub fn is_full(&self) -> bool {
        !self.is_empty()
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
    fn try_select(&self, token: &mut Token) -> bool {
        match self.try_recv() {
            Ok(msg) => {
                token.tick = Some(msg);
                true
            }
            Err(TryRecvError::Disconnected) => {
                token.tick = None;
                true
            }
            Err(TryRecvError::Empty) => false,
        }
    }

    #[inline]
    fn deadline(&self) -> Option<Instant> {
        Some(self.inner.lock().next_tick)
    }

    #[inline]
    fn register(&self, _oper: Operation, _cx: &Context) -> bool {
        self.is_ready()
    }

    #[inline]
    fn unregister(&self, _oper: Operation) {}

    #[inline]
    fn accept(&self, token: &mut Token, _cx: &Context) -> bool {
        self.try_select(token)
    }

    #[inline]
    fn is_ready(&self) -> bool {
        !self.is_empty()
    }

    #[inline]
    fn watch(&self, _oper: Operation, _cx: &Context) -> bool {
        self.is_ready()
    }

    #[inline]
    fn unwatch(&self, _oper: Operation) {}

    #[inline]
    fn state(&self) -> usize {
        // Return the index of the next message to be delivered to the channel.
        let inner = self.inner.lock();
        let index = if Instant::now() < inner.next_tick {
            inner.index
        } else {
            inner.index + Wrapping(1)
        };
        index.0
    }
}
