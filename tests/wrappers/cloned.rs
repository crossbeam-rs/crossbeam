//! Channels with senders and receivers cloned immediately upon construction.
//!
//! Some channel types might take advantage of the fact that the first sender or receiver was never
//! cloned, so this wrapper prevents such optimizations right away.

use std::ops::Deref;
use std::time::{Duration, Instant};

use crossbeam_channel as cc;

pub use self::cc::{RecvError, RecvTimeoutError, TryRecvError};
pub use self::cc::{SendError, SendTimeoutError, TrySendError};
pub use self::cc::Select;

pub struct Sender<T>(pub cc::Sender<T>);

pub struct Receiver<T>(pub cc::Receiver<T>);

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        Sender(self.0.clone())
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Receiver<T> {
        Receiver(self.0.clone())
    }
}

impl<T> Deref for Receiver<T> {
    type Target = cc::Receiver<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Sender<T> {
    type Target = cc::Sender<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let (s, r) = cc::bounded(cap);
    (Sender(s.clone()), Receiver(r.clone()))
}

pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let (s, r) = cc::unbounded();
    (Sender(s.clone()), Receiver(r.clone()))
}

pub fn after(dur: Duration) -> Receiver<Instant> {
    let r = cc::after(dur);
    Receiver(r.clone())
}

pub fn tick(dur: Duration) -> Receiver<Instant> {
    let r = cc::tick(dur);
    Receiver(r.clone())
}
