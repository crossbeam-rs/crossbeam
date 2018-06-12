//! Native interface with the default behavior.
//!
//! This wrapper does not try to provoke or prevent any optimizations.

use std::ops::Deref;
use std::time::{Duration, Instant};

use channel;

pub struct Sender<T>(pub channel::Sender<T>);

pub struct Receiver<T>(pub channel::Receiver<T>);

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
    type Target = channel::Receiver<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Deref for Sender<T> {
    type Target = channel::Sender<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let (s, r) = channel::bounded(cap);
    (Sender(s), Receiver(r))
}

pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let (s, r) = channel::unbounded();
    (Sender(s), Receiver(r))
}

pub fn after(dur: Duration) -> Receiver<Instant> {
    let r = channel::after(dur);
    Receiver(r)
}

pub fn tick(dur: Duration) -> Receiver<Instant> {
    let r = channel::tick(dur);
    Receiver(r)
}
