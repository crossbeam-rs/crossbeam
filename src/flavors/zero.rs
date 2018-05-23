//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::time::Instant;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use exchanger::{ExchangeError, Exchanger};
use select::CaseId;

/// A zero-capacity channel.
pub struct Channel<T> {
    /// The internal two-sided exchanger.
    exchanger: Exchanger<Option<T>>,
}

impl<T> Channel<T> {
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
            Err(ExchangeError::Disconnected(msg)) => Err(TrySendError::Disconnected(msg.unwrap())),
            Err(ExchangeError::Timeout(msg)) => Err(TrySendError::Full(msg.unwrap())),
        }
    }

    /// Attempts to send `msg` into the channel until the specified `deadline`.
    pub fn send_until(
        &self,
        msg: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<(), SendTimeoutError<T>> {
        match self.exchanger
            .left()
            .exchange_until(Some(msg), deadline, case_id)
        {
            Ok(_) => Ok(()),
            Err(ExchangeError::Disconnected(msg)) => {
                Err(SendTimeoutError::Disconnected(msg.unwrap()))
            }
            Err(ExchangeError::Timeout(msg)) => Err(SendTimeoutError::Timeout(msg.unwrap())),
        }
    }

    /// Attempts to receive a message from channel.
    pub fn try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.exchanger.right().try_exchange(None, case_id) {
            Ok(msg) => Ok(msg.unwrap()),
            Err(ExchangeError::Disconnected(_)) => Err(TryRecvError::Disconnected),
            Err(ExchangeError::Timeout(_)) => Err(TryRecvError::Empty),
        }
    }

    /// Attempts to receive a message from the channel until the specified `deadline`.
    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        match self.exchanger
            .right()
            .exchange_until(None, deadline, case_id)
        {
            Ok(msg) => Ok(msg.unwrap()),
            Err(ExchangeError::Disconnected(_)) => Err(RecvTimeoutError::Disconnected),
            Err(ExchangeError::Timeout(_)) => Err(RecvTimeoutError::Timeout),
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

    /// Disconnects the channel and wakes up all currently blocked operations on it.
    pub fn disconnect(&self) -> bool {
        self.exchanger.disconnect()
    }

    /// Returns `true` if the channel is disconnected.
    pub fn is_disconnected(&self) -> bool {
        self.exchanger.is_disconnected()
    }
}
