#![feature(thread_id)]

extern crate coco;
extern crate crossbeam;
extern crate either;

use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::time::{Duration, Instant};

mod async;
mod blocking;
mod sync;
mod zero;

// TODO: Use a oneshot optimization

// TODO: iterators

// TODO: len() (add counters to each Node in the async version) (see go implementation)
// TODO: is_empty()
// TODO: make Sender and Receiver Send + Sync

// TODO: impl Error and Display for these errors
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

    fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>
    ) -> Result<(), SendTimeoutError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => unimplemented!(),
            Flavor::Sync(ref q) => unimplemented!(),
            Flavor::Zero(ref q) => q.send_until(value, deadline),
        }
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.send(value),
            Flavor::Sync(ref q) => unimplemented!(),
            Flavor::Zero(ref q) => unimplemented!(),
        }
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        self.send_until(value, Some(Instant::now() + dur))
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_send(value),
            Flavor::Sync(ref q) => q.try_send(value),
            Flavor::Zero(ref q) => q.try_send(value),
        }
    }

    fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        match self.flavor {
            Flavor::Async(ref q) => q.recv_until(deadline),
            Flavor::Sync(ref q) => unimplemented!(),
            Flavor::Zero(ref q) => q.recv_until(deadline),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        if let Ok(v) = self.recv_until(None) {
            Ok(v)
        } else {
            Err(RecvError)
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        match self.flavor {
            Flavor::Async(ref q) => q.recv_timeout(dur),
            Flavor::Sync(ref q) => unimplemented!(),
            Flavor::Zero(ref q) => unimplemented!(),
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

pub struct Select {
    //v: Cell<Vec<blocking::Token>>,
}

impl Select {
    pub fn new() -> Self {
        Select {
            // v: vec![],
        }
    }

    pub fn with_timeout(dur: Duration) -> Self {
        unimplemented!()
    }

    pub fn iter(&self) -> Iter {
        unimplemented!()
    }
}

pub struct Iter<'a> {
    parent: &'a Select,
}

pub struct Step<'a> {
    parent: &'a Select,
}

impl<'a> Step<'a> {
    pub fn send<T>(&self, tx: &Sender<T>, value: T) -> Result<(), TrySendError<T>> {
        unimplemented!()
    }

    pub fn recv<T>(&self, rx: &Receiver<T>) -> Result<T, TryRecvError> {
        unimplemented!()
    }

    pub fn all_blocked(&self) -> bool {
        unimplemented!()
    }

    pub fn all_closed(&self) -> bool {
        unimplemented!()
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = Step<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        None
    }
}

impl<'a> IntoIterator for &'a Select {
    type Item = Step<'a>;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        unimplemented!()
    }
}

fn foo() {
    let (tx1, rx1) = async::<i32>();
    let (tx2, rx2) = async::<i32>();

    // for sel in &Select::with_timeout(Duration::from_millis(100)) {
    // for s in &Select::new() {
    //     if let Ok(_) = rx1.poll(s) {
    //
    //     }
    //     if let Ok(value) = rx2.poll(s) {
    //
    //     }
    //     if s.timed_out() {
    //
    //     }
    //     if s.all_blocked() {
    //
    //     }
    //     if s.all_disconnected() {
    //
    //     }
    // }
}
