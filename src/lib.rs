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
mod select;
mod sync;
mod zero;

// TODO: Perhaps channel::unbounded() and channel::bounded(cap)

// TODO: iterators

// TODO: len() (add counters to each Node in the async version) (see go implementation)
// TODO: is_empty()
// TODO: is_full()
// TODO: capacity()

// TODO: make Sender and Receiver Send + Sync

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

trait Channel<T> {
    fn send(&self, value: T) -> Result<(), SendError<T>>;
    fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>>;
    fn try_send(&self, value: T) -> Result<(), TrySendError<T>>;

    fn recv(&self) -> Result<T, RecvError>;
    fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError>;
    fn try_recv(&self) -> Result<T, TryRecvError>;

    fn len(&self) -> usize;
    fn close(&self) -> bool;
    fn is_closed(&self) -> bool;

    fn subscribe(&self);
    fn unsubscribe(&self);
    fn is_ready(&self) -> bool;
    fn id(&self) -> usize;
}

enum Flavor<T> {
    Async(async::Queue<T>),
    Sync(sync::Queue<T>),
    Zero(zero::Queue<T>),
}

struct Inner<T> {
    senders: AtomicUsize,
    receivers: AtomicUsize,
    queue: Flavor<T>,
}

pub struct Sender<T>(Arc<Inner<T>>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> Sender<T> {
    fn new(q: Arc<Inner<T>>) -> Self {
        q.senders.fetch_add(1, SeqCst);
        Sender(q)
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        match self.0.queue {
            Flavor::Async(ref q) => q.send(value),
            Flavor::Sync(ref q) => q.send(value),
            Flavor::Zero(ref q) => q.send(value),
        }
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        match self.0.queue {
            Flavor::Async(ref q) => q.send_timeout(value, dur),
            Flavor::Sync(ref q) => q.send_timeout(value, dur),
            Flavor::Zero(ref q) => q.send_timeout(value, dur),
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.0.queue {
            Flavor::Async(ref q) => q.try_send(value),
            Flavor::Sync(ref q) => q.try_send(value),
            Flavor::Zero(ref q) => q.try_send(value),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, SeqCst) == 1 {
            match self.0.queue {
                Flavor::Async(ref q) => q.close(),
                Flavor::Sync(ref q) => q.close(),
                Flavor::Zero(ref q) => q.close(),
            };
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender::new(self.0.clone())
    }
}

pub struct Receiver<T>(Arc<Inner<T>>);

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> Receiver<T> {
    fn new(q: Arc<Inner<T>>) -> Self {
        q.receivers.fetch_add(1, SeqCst);
        Receiver(q)
    }

    fn channel(&self) -> &Channel<T> {
        match self.0.queue {
            Flavor::Async(ref q) => q,
            Flavor::Sync(ref q) => q,
            Flavor::Zero(ref q) => unimplemented!(),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        match self.0.queue {
            Flavor::Async(ref q) => q.recv(),
            Flavor::Sync(ref q) => q.recv(),
            Flavor::Zero(ref q) => q.recv(),
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        match self.0.queue {
            Flavor::Async(ref q) => q.recv_timeout(dur),
            Flavor::Sync(ref q) => q.recv_timeout(dur),
            Flavor::Zero(ref q) => q.recv_timeout(dur),
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.0.queue {
            Flavor::Async(ref q) => q.try_recv(),
            Flavor::Sync(ref q) => q.try_recv(),
            Flavor::Zero(ref q) => q.try_recv(),
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if self.0.receivers.fetch_sub(1, SeqCst) == 1 {
            match self.0.queue {
                Flavor::Async(ref q) => q.close(),
                Flavor::Sync(ref q) => q.close(),
                Flavor::Zero(ref q) => q.close(),
            };
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Receiver::new(self.0.clone())
    }
}

pub fn async<T>() -> (Sender<T>, Receiver<T>) {
    let q = Arc::new(Inner {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        queue: Flavor::Async(async::Queue::new()),
    });
    (Sender::new(q.clone()), Receiver::new(q))
}

pub fn sync<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    let q = Arc::new(Inner {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        queue: if size == 0 {
            Flavor::Zero(zero::Queue::new())
        } else {
            Flavor::Sync(sync::Queue::with_capacity(size))
        },
    });
    (Sender::new(q.clone()), Receiver::new(q))
}
