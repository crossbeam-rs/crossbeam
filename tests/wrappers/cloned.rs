///! TODO

use std::ops::Deref;
use std::time::{Duration, Instant};

use channel;

#[derive(Clone)]
pub struct Sender<T>(pub channel::Sender<T>);

#[derive(Clone)]
pub struct Receiver<T>(pub channel::Receiver<T>);

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

#[allow(dead_code)]
pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let (s, r) = channel::bounded(cap);
    (Sender(s.clone()), Receiver(r.clone()))
}

#[allow(dead_code)]
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let (s, r) = channel::unbounded();
    (Sender(s.clone()), Receiver(r.clone()))
}

pub fn after(dur: Duration) -> Receiver<Instant> {
    let r = channel::after(dur);
    Receiver(r.clone())
}
