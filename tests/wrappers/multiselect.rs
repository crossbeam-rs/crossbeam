use std::option;
use std::time::{Duration, Instant};

use channel;

pub struct Sender<T>(channel::Sender<T>);
pub struct Receiver<T>(channel::Receiver<T>);

impl<T> Sender<T> {
    pub fn send(&self, msg: T) {
        // Two cases to prevent internal optimizations from triggering (if they exist).
        select! {
            send(self.0, msg) => {}
            send(self.0, msg) => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.0.is_full()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn capacity(&self) -> Option<usize> {
        self.0.capacity()
    }
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Option<T> {
        // Two cases to prevent internal optimizations from triggering (if they exist).
        select! {
            recv(self.0, msg) => msg,
            recv(self.0, msg) => msg,
            default => None,
        }
    }

    pub fn recv(&self) -> Option<T> {
        // Two cases to prevent internal optimizations from triggering (if they exist).
        select! {
            recv(self.0, msg) => msg,
            recv(self.0, msg) => msg,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.0.is_full()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn capacity(&self) -> Option<usize> {
        self.0.capacity()
    }
}

impl<'a, T> channel::internal::codegen::SendArgument<'a, T> for &'a Sender<T> {
    type Iter = option::IntoIter<&'a channel::Sender<T>>;

    fn __as_send_argument(&'a self) -> Self::Iter {
        Some(&self.0).into_iter()
    }
}

impl<'a, T> channel::internal::codegen::RecvArgument<'a, T> for &'a Receiver<T> {
    type Iter = option::IntoIter<&'a channel::Receiver<T>>;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        Some(&self.0).into_iter()
    }
}

#[allow(dead_code)]
pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let (s, r) = channel::bounded(cap);
    (Sender(s), Receiver(r))
}

#[allow(dead_code)]
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let (s, r) = channel::unbounded();
    (Sender(s), Receiver(r))
}

pub fn after(dur: Duration) -> Receiver<Instant> {
    let r = channel::after(dur);
    Receiver(r)
}
