//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::time::Instant;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use exchanger::{Exchanger, ExchangeError};
use select::CaseId;

/// A zero-capacity channel.
pub struct Channel<T> {
    /// The internal two-sided exchanger.
    exchanger: Exchanger<Option<T>>,
}

impl<T> Channel<T> {
    /// Returns a new zero-capacity channel.
    pub fn new() -> Self {
        Channel { exchanger: Exchanger::new() }
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
    pub fn fulfill_send(&self, value: T) {
        self.exchanger.left().fulfill(Some(value));
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

    /// Attempts to send `value` into the channel.
    pub fn try_send(&self, value: T, case_id: CaseId) -> Result<(), TrySendError<T>> {
        match self.exchanger.left().try_exchange(Some(value), case_id) {
            Ok(_) => Ok(()),
            Err(ExchangeError::Disconnected(v)) => Err(TrySendError::Disconnected(v.unwrap())),
            Err(ExchangeError::Timeout(v)) => Err(TrySendError::Full(v.unwrap())),
        }
    }

    /// Attempts to send `value` into the channel until the specified `deadline`.
    pub fn send_until(
        &self,
        value: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<(), SendTimeoutError<T>> {
        match self.exchanger.left().exchange_until(Some(value), deadline, case_id) {
            Ok(_) => Ok(()),
            Err(ExchangeError::Disconnected(v)) => Err(SendTimeoutError::Disconnected(v.unwrap())),
            Err(ExchangeError::Timeout(v)) => Err(SendTimeoutError::Timeout(v.unwrap())),
        }
    }

    /// Attempts to receive a value from channel.
    pub fn try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.exchanger.right().try_exchange(None, case_id) {
            Ok(v) => Ok(v.unwrap()),
            Err(ExchangeError::Disconnected(_)) => Err(TryRecvError::Disconnected),
            Err(ExchangeError::Timeout(_)) => Err(TryRecvError::Empty),
        }
    }

    /// Attempts to receive a value from the channel until the specified `deadline`.
    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        match self.exchanger.right().exchange_until(None, deadline, case_id) {
            Ok(v) => Ok(v.unwrap()),
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

    /// Closes the channel.
    pub fn close(&self) -> bool {
        self.exchanger.close()
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.exchanger.is_closed()
    }
}
