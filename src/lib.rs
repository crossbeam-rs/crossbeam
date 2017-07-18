#![feature(thread_id)]

extern crate coco;
extern crate crossbeam;
extern crate rand;

use std::collections::VecDeque;
use std::ptr;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use monitor::Monitor;

use rand::{Rng, thread_rng};

mod async;
mod monitor;
mod sync;
mod zero;

// TODO: Perhaps channel::unbounded() and channel::bounded(cap)

// TODO: Use a oneshot optimization

// TODO: iterators

// TODO: len() (add counters to each Node in the async version) (see go implementation)
// TODO: is_empty()
// TODO: make Sender and Receiver Send + Sync

// TODO: impl Error and Display for these errors

// TODO: Perhaps another channel::oneshot() variant?

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SendTimeoutError<T> {
    Timeout(T),
    Disconnected(T),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T> {
    Full(T),
    Disconnected(T),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct RecvError;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RecvTimeoutError {
    Timeout,
    Disconnected,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TryRecvError {
    Empty,
    Disconnected,
}

enum Flavor<T> {
    Async(async::Queue<T>),
    Sync(sync::Queue<T>),
    Zero(zero::Queue<T>),
}

pub struct Queue<T> {
    flavor: Flavor<T>,
    senders: AtomicUsize,
    receivers: AtomicUsize,
}

impl<T> Queue<T> {
    pub fn async() -> Self {
        Queue {
            flavor: Flavor::Async(async::Queue::new()),
            senders: AtomicUsize::new(0),
            receivers: AtomicUsize::new(0),
        }
    }

    pub fn sync(cap: usize) -> Self {
        let flavor = if cap == 0 {
            Flavor::Zero(zero::Queue::new())
        } else {
            Flavor::Sync(sync::Queue::with_capacity(cap))
        };

        Queue {
            flavor: flavor,
            senders: AtomicUsize::new(0),
            receivers: AtomicUsize::new(0),
        }
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.send(value),
            Flavor::Sync(ref q) => q.send(value),
            Flavor::Zero(ref q) => q.send(value),
        }
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.send_timeout(value, dur),
            Flavor::Sync(ref q) => q.send_timeout(value, dur),
            Flavor::Zero(ref q) => q.send_timeout(value, dur),
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_send(value),
            Flavor::Sync(ref q) => q.try_send(value),
            Flavor::Zero(ref q) => q.try_send(value),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        match self.flavor {
            Flavor::Async(ref q) => q.recv(),
            Flavor::Sync(ref q) => q.recv(),
            Flavor::Zero(ref q) => q.recv(),
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        match self.flavor {
            Flavor::Async(ref q) => q.recv_timeout(dur),
            Flavor::Sync(ref q) => q.recv_timeout(dur),
            Flavor::Zero(ref q) => q.recv_timeout(dur),
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_recv(),
            Flavor::Sync(ref q) => q.try_recv(),
            Flavor::Zero(ref q) => q.try_recv(),
        }
    }

    pub fn close(&self) -> bool {
        match self.flavor {
            Flavor::Async(ref q) => q.close(),
            Flavor::Sync(ref q) => q.close(),
            Flavor::Zero(ref q) => q.close(),
        }
    }

    pub fn is_closed(&self) -> bool {
        match self.flavor {
            Flavor::Async(ref q) => q.is_closed(),
            Flavor::Sync(ref q) => q.is_closed(),
            Flavor::Zero(ref q) => q.is_closed(),
        }
    }

    fn receivers(&self) -> &Monitor {
        match self.flavor {
            Flavor::Async(ref q) => unimplemented!(),
            Flavor::Sync(ref q) => &q.receivers,
            Flavor::Zero(ref q) => unimplemented!(),
        }
    }

    fn is_ready(&self) -> bool {
        match self.flavor {
            Flavor::Async(ref q) => unimplemented!(),
            Flavor::Sync(ref q) => q.is_ready(),
            Flavor::Zero(ref q) => unimplemented!(),
        }
    }
}

pub struct Sender<T>(Arc<Queue<T>>);

impl<T> Sender<T> {
    fn new(q: Arc<Queue<T>>) -> Self {
        q.senders.fetch_add(1, SeqCst);
        Sender(q)
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.0.send(value)
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        self.0.send_timeout(value, dur)
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        self.0.try_send(value)
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, SeqCst) == 1 {
            self.0.close();
        }
    }
}

pub struct Receiver<T>(Arc<Queue<T>>);

impl<T> Receiver<T> {
    fn new(q: Arc<Queue<T>>) -> Self {
        q.receivers.fetch_add(1, SeqCst);
        Receiver(q)
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        self.0.recv()
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        self.0.recv_timeout(dur)
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.0.try_recv()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if self.0.receivers.fetch_sub(1, SeqCst) == 1 {
            self.0.close();
        }
    }
}

pub fn async<T>() -> (Sender<T>, Receiver<T>) {
    let q = Arc::new(Queue::async());
    (Sender::new(q.clone()), Receiver::new(q))
}

pub fn sync<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    let q = Arc::new(Queue::sync(size));
    (Sender::new(q.clone()), Receiver::new(q))
}
