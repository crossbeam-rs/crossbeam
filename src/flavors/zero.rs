//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::time::Instant;

use err::{TryRecvError, TrySendError};
use exchanger::{ExchangeError, Exchanger};
use select::CaseId;

/// A zero-capacity channel.
pub struct Channel<T> {
    /// The internal two-sided exchanger.
    exchanger: Exchanger<Option<T>>,
}

impl<T> Channel<T> {
    pub fn sel_try_recv(&self) -> Option<usize> {
        self.exchanger.right().sel_try_exchange()
    }

    pub unsafe fn finish_recv(&self, token: usize) -> Option<T> {
        if token == 0 {
            None
        } else if token == 1 {
            Some(self.fulfill_recv())
        } else {
            Some(self.exchanger.right().finish_exchange(token, None).unwrap())
        }
    }

    pub fn sel_try_send(&self) -> Option<usize> {
        self.exchanger.left().sel_try_exchange()
    }

    pub unsafe fn finish_send(&self, token: usize, msg: T) {
        if token == 1 {
            self.fulfill_send(msg);
        } else {
            self.exchanger.right().finish_exchange(token, Some(msg));
        }
    }

    /// Returns a new zero-capacity channel.
    pub fn new() -> Self {
        Channel {
            exchanger: Exchanger::new(),
        }
    }

    /// Promises a send operation.
    pub fn promise_send(&self, case_id: CaseId) {
        self.exchanger.left().promise(case_id);
    }

    /// Revokes the promised send operation.
    pub fn revoke_send(&self, case_id: CaseId) {
        self.exchanger.left().revoke(case_id);
    }

    /// Fulfills the promised send operation.
    pub fn fulfill_send(&self, msg: T) {
        self.exchanger.left().fulfill(Some(msg));
    }

    /// Promises a receive operation.
    pub fn promise_recv(&self, case_id: CaseId) {
        self.exchanger.right().promise(case_id);
    }

    /// Revokes the promised receive operation.
    pub fn revoke_recv(&self, case_id: CaseId) {
        self.exchanger.right().revoke(case_id);
    }

    /// Fulfills the promised receive operation.
    pub fn fulfill_recv(&self) -> T {
        self.exchanger.right().fulfill(None).unwrap()
    }

    /// Attempts to send `msg` into the channel.
    pub fn try_send(&self, msg: T, case_id: CaseId) -> Result<(), TrySendError<T>> {
        match self.exchanger.left().try_exchange(Some(msg), case_id) {
            Ok(_) => Ok(()),
            Err(ExchangeError::Closed(msg)) => Err(TrySendError::Closed(msg.unwrap())),
            Err(ExchangeError::Timeout(msg)) => Err(TrySendError::Full(msg.unwrap())),
        }
    }

    /// Attempts to send `msg` into the channel.
    pub fn send(&self, msg: T, case_id: CaseId) {
        match self.exchanger
            .left()
            .exchange_until(Some(msg), None, case_id)
        {
            Ok(_) => (),
            Err(ExchangeError::Closed(msg)) => panic!(), // TODO: delete this case
            Err(ExchangeError::Timeout(msg)) => panic!(), // TODO: cannot happen?
        }
    }

    /// Attempts to receive a message from channel.
    pub fn try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.exchanger.right().try_exchange(None, case_id) {
            Ok(msg) => Ok(msg.unwrap()),
            Err(ExchangeError::Closed(_)) => Err(TryRecvError::Closed),
            Err(ExchangeError::Timeout(_)) => Err(TryRecvError::Empty),
        }
    }

    /// Attempts to receive a message from the channel.
    pub fn recv(
        &self,
        case_id: CaseId,
    ) -> Option<T> {
        match self.exchanger
            .right()
            .exchange_until(None, None, case_id)
        {
            Ok(msg) => Some(msg.unwrap()),
            Err(ExchangeError::Closed(_)) => None,
            Err(ExchangeError::Timeout(_)) => panic!(), // TODO: cannot happeN?
        }
    }

    /// Returns `true` if there is a waiting sender.
    pub fn can_recv(&self) -> bool {
        self.exchanger.left().can_notify()
    }

    /// Returns `true` if there is a waiting receiver.
    pub fn can_send(&self) -> bool {
        self.exchanger.right().can_notify()
    }

    /// Closes the channel and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        self.exchanger.close()
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.exchanger.is_closed()
    }
}
