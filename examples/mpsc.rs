//! A reimplementation of the `std::sync::mpsc` interface using `crossbeam-channel`.
//!
//! This is a channel implementation mimicking MPSC channels from the standard library, but the
//! internals actually use `crossbeam-channel`. There's an auxilliary channel `disconnected`, which
//! becomes closed once the receiver gets dropped, thus notifying the senders that the MPSC channel
//! is disconnected.

#[macro_use]
extern crate crossbeam_channel as channel;

use std::thread;
use std::time::Duration;

// Re-export the error types.
pub use std::sync::mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError, TrySendError};

#[derive(Clone, Debug)]
pub struct Sender<T> {
    inner: channel::Sender<T>,
    disconnected: channel::Receiver<()>,
}

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        // Check if all receivers have been dropped.
        select! {
            recv(self.disconnected) => return Err(SendError(t)),
            default => {}
        }

        // Send the message or wait for all receivers to get dropped, whatever happens first.
        select! {
            send(self.inner, t) => Ok(()),
            recv(self.disconnected) => Err(SendError(t)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SyncSender<T> {
    inner: channel::Sender<T>,
    disconnected: channel::Receiver<()>,
}

impl<T> SyncSender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        // Check if all receivers have been dropped.
        select! {
            recv(self.disconnected) => return Err(SendError(t)),
            default => {}
        }

        // Send the message or wait for all receivers to get dropped, whatever happens first.
        select! {
            send(self.inner, t) => Ok(()),
            recv(self.disconnected) => Err(SendError(t)),
        }
    }

    pub fn try_send(&self, t: T) -> Result<(), TrySendError<T>> {
        // Check if all receivers have been dropped.
        select! {
            recv(self.disconnected) => return Err(TrySendError::Disconnected(t)),
            default => {}
        }

        // Send the message or wait for all receivers to get dropped, whatever happens first.
        select! {
            send(self.inner, t) => Ok(()),
            recv(self.disconnected) => Err(TrySendError::Disconnected(t)),
            default => Err(TrySendError::Full(t)),
        }
    }
}

#[derive(Debug)]
pub struct Receiver<T> {
    inner: channel::Receiver<T>,
    disconnected: channel::Sender<()>,
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        select! {
            recv(self.inner, msg) => match msg {
                None => Err(TryRecvError::Disconnected),
                Some(msg) => Ok(msg),
            }
            default => Err(TryRecvError::Empty),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        select! {
            recv(self.inner, msg) => match msg {
                None => Err(RecvError),
                Some(msg) => Ok(msg),
            }
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        select! {
            recv(self.inner, msg) => match msg {
                None => Err(RecvTimeoutError::Disconnected),
                Some(msg) => Ok(msg),
            }
            recv(channel::after(timeout)) => Err(RecvTimeoutError::Timeout),
        }
    }

    pub fn iter(&self) -> Iter<T> {
        Iter { inner: self }
    }

    pub fn try_iter(&self) -> TryIter<T> {
        TryIter { inner: self }
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter { inner: self }
    }
}

#[derive(Debug)]
pub struct TryIter<'a, T: 'a> {
    inner: &'a Receiver<T>,
}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.try_recv().ok()
    }
}

#[derive(Debug)]
pub struct Iter<'a, T: 'a> {
    inner: &'a Receiver<T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.recv().ok()
    }
}

#[derive(Debug)]
pub struct IntoIter<T> {
    inner: Receiver<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.recv().ok()
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (s1, r1) = channel::unbounded();
    let (s2, r2) = channel::bounded(1);

    let s = Sender {
        inner: s1,
        disconnected: r2,
    };
    let r = Receiver {
        inner: r1,
        disconnected: s2,
    };
    (s, r)
}

pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
    let (s1, r1) = channel::bounded(bound);
    let (s2, r2) = channel::bounded(1);

    let s = SyncSender {
        inner: s1,
        disconnected: r2,
    };
    let r = Receiver {
        inner: r1,
        disconnected: s2,
    };
    (s, r)
}

fn main() {
    // Example #1:
    let (tx, rx) = channel();
    thread::spawn(move || tx.send(10).unwrap());
    assert_eq!(rx.recv().unwrap(), 10);

    // Example #2:
    let (tx, rx) = sync_channel::<i32>(0);
    thread::spawn(move || tx.send(53).unwrap());
    rx.recv().unwrap();
}
