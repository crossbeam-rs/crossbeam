//! TODO Channel that delivers messages periodically.
//!
//! Messages cannot be sent into this kind of channel; they are materialized on demand.

use std::marker::PhantomData;
use std::time::Instant;

use context::Context;
use err::{RecvTimeoutError, TryRecvError};
use select::{Operation, SelectHandle, Token};
use utils;

/// This flavor doesn't need a token.
pub type NeverToken = ();

/// TODO Channel that delivers messages periodically.
pub struct Channel<T> {
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    /// TODO Creates a channel that delivers messages periodically.
    #[inline]
    pub fn new() -> Self {
        Channel {
            _marker: PhantomData,
        }
    }

    /// Attempts to receive a message without blocking.
    #[inline]
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        Err(TryRecvError::Empty)
    }

    /// Receives a message from the channel.
    #[inline]
    pub fn recv(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        utils::sleep_until(deadline);
        Err(RecvTimeoutError::Timeout)
    }

    /// Reads a message from the channel.
    #[inline]
    pub unsafe fn read(&self, _token: &mut Token) -> Result<T, ()> {
        Err(())
    }

    /// Returns `true` if the channel is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        true
    }

    /// Returns `true` if the channel is full.
    #[inline]
    pub fn is_full(&self) -> bool {
        true
    }

    /// Returns the number of messages in the channel.
    #[inline]
    pub fn len(&self) -> usize {
        0
    }

    /// Returns the capacity of the channel.
    #[inline]
    pub fn capacity(&self) -> Option<usize> {
        Some(0)
    }
}

impl<T> Clone for Channel<T> {
    #[inline]
    fn clone(&self) -> Channel<T> {
        Channel::new()
    }
}

impl<T> SelectHandle for Channel<T> {
    #[inline]
    fn try(&self, _token: &mut Token) -> bool {
        false
    }

    #[inline]
    fn retry(&self, _token: &mut Token) -> bool {
        false
    }

    #[inline]
    fn deadline(&self) -> Option<Instant> {
        None
    }

    #[inline]
    fn register(&self, _token: &mut Token, _oper: Operation, _cx: &Context) -> bool {
        true
    }

    #[inline]
    fn unregister(&self, _oper: Operation) {}

    #[inline]
    fn accept(&self, _token: &mut Token, _cx: &Context) -> bool {
        false
    }

    #[inline]
    fn state(&self) -> usize {
        0
    }
}
