//! A reimplementation of the `std::sync::mpsc` interface using `crossbeam-channel`.
//!
//! This is a channel implementation mimicking MPSC channels from the standard library, but the
//! internals actually use `crossbeam-channel`. There's an auxilliary channel `disconnected`, which
//! becomes closed once the receiver gets dropped, thus notifying the senders that the MPSC channel
//! is disconnected.

#[macro_use]
extern crate crossbeam_channel as channel;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError, TrySendError};
use std::thread;
use std::time::Duration;

pub struct Sender<T> {
    pub inner: channel::Sender<T>,
    disconnected: channel::Receiver<()>,
    is_disconnected: Arc<AtomicBool>,
}

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        if self.is_disconnected.load(Ordering::SeqCst) {
            Err(SendError(t))
        } else {
            self.inner.send(t);
            Ok(())
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        Sender {
            inner: self.inner.clone(),
            disconnected: self.disconnected.clone(),
            is_disconnected: self.is_disconnected.clone(),
        }
    }
}

pub struct SyncSender<T> {
    pub inner: channel::Sender<T>,
    disconnected: channel::Receiver<()>,
    is_disconnected: Arc<AtomicBool>,
}

impl<T> SyncSender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        if self.is_disconnected.load(Ordering::SeqCst) {
            Err(SendError(t))
        } else {
            select! {
                send(self.inner, t) => Ok(()),
                default => {
                    select! {
                        send(self.inner, t) => Ok(()),
                        recv(self.disconnected) => Err(SendError(t)),
                    }
                }
            }
        }
    }

    pub fn try_send(&self, t: T) -> Result<(), TrySendError<T>> {
        if self.is_disconnected.load(Ordering::SeqCst) {
            Err(TrySendError::Disconnected(t))
        } else {
            select! {
                send(self.inner, t) => Ok(()),
                default => Err(TrySendError::Full(t)),
            }
        }
    }
}

impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> SyncSender<T> {
        SyncSender {
            inner: self.inner.clone(),
            disconnected: self.disconnected.clone(),
            is_disconnected: self.is_disconnected.clone(),
        }
    }
}

pub struct Receiver<T> {
    pub inner: channel::Receiver<T>,
    _disconnected: channel::Sender<()>,
    is_disconnected: Arc<AtomicBool>,
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
        match self.inner.recv() {
            None => Err(RecvError),
            Some(msg) => Ok(msg),
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        select! {
            recv(self.inner, msg) => match msg {
                None => Err(RecvTimeoutError::Disconnected),
                Some(msg) => Ok(msg),
            }
            default => {
                select! {
                    recv(self.inner, msg) => match msg {
                        None => Err(RecvTimeoutError::Disconnected),
                        Some(msg) => Ok(msg),
                    }
                    recv(channel::after(timeout)) => Err(RecvTimeoutError::Timeout),
                }
            }
        }
    }

    pub fn iter(&self) -> Iter<T> {
        Iter { inner: self }
    }

    pub fn try_iter(&self) -> TryIter<T> {
        TryIter { inner: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.is_disconnected.store(true, Ordering::SeqCst);
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

pub struct TryIter<'a, T: 'a> {
    inner: &'a Receiver<T>,
}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.try_recv().ok()
    }
}

pub struct Iter<'a, T: 'a> {
    inner: &'a Receiver<T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.recv().ok()
    }
}

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
    let is_disconnected = Arc::new(AtomicBool::new(false));

    let s = Sender {
        inner: s1,
        disconnected: r2,
        is_disconnected: is_disconnected.clone(),
    };
    let r = Receiver {
        inner: r1,
        _disconnected: s2,
        is_disconnected,
    };
    (s, r)
}

pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
    let (s1, r1) = channel::bounded(bound);
    let (s2, r2) = channel::bounded(1);
    let is_disconnected = Arc::new(AtomicBool::new(false));

    let s = SyncSender {
        inner: s1,
        disconnected: r2,
        is_disconnected: is_disconnected.clone(),
    };
    let r = Receiver {
        inner: r1,
        _disconnected: s2,
        is_disconnected,
    };
    (s, r)
}

macro_rules! mpsc_select {
    (
        $($name:pat = $rx:ident.$meth:ident() => $code:expr),+
    ) => ({
        select! {
            $(
                $meth(($rx).inner, msg) => {
                    let $name = match msg {
                        None => Err(::std::sync::mpsc::RecvError),
                        Some(msg) => Ok(msg),
                    };
                    $code
                }
            )+
        }
    })
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

    // Example #3:
    let (_tx1, rx1) = channel::<i32>();
    let (tx2, rx2) = channel::<i32>();
    tx2.send(0).unwrap();
    mpsc_select! {
        _ = rx1.recv() => panic!(),
        m = rx2.recv() => assert_eq!(m, Ok(0))
    }
}
