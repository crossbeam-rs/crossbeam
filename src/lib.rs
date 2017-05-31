extern crate coco;
extern crate either;

use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::time::{Duration, Instant};

mod async;
pub mod buffered;
mod rendezvous;

// TODO: len() (add counters to each Node in the async version)
// TODO: is_empty()
// TODO: make Sender and Receiver Send + Sync

#[derive(Debug)]
pub struct SendError<T>(pub T);

#[derive(Debug)]
pub enum SendTimeoutError<T> {
    Timeout(T),
    Disconnected(T),
}

#[derive(Debug)]
pub enum TrySendError<T> {
    Full(T),
    Disconnected(T),
}

#[derive(Debug)]
pub struct RecvError;

#[derive(Debug)]
pub enum RecvTimeoutError {
    Timeout,
    Disconnected,
}

#[derive(Debug)]
pub enum TryRecvError {
    Empty,
    Disconnected,
}

enum Flavor<T> {
    Async(async::Queue<T>),
    Buffered(buffered::Queue<T>),
    Rendezvous(rendezvous::Queue<T>),
}

pub struct Queue<T> {
    flavor: Flavor<T>,
    lock: Mutex<()>,
    cond: Condvar,
    blocked: AtomicUsize,
    senders: AtomicUsize,
    receivers: AtomicUsize,
}

impl<T> Queue<T> {
    pub fn async() -> Self {
        Queue {
            flavor: Flavor::Async(async::Queue::new()),
            lock: Mutex::new(()),
            cond: Condvar::new(),
            blocked: AtomicUsize::new(0),
            senders: AtomicUsize::new(0),
            receivers: AtomicUsize::new(0),
        }
    }

    pub fn sync(cap: usize) -> Self {
        let flavor = if cap == 0 {
            Flavor::Rendezvous(rendezvous::Queue::new())
        } else {
            Flavor::Buffered(buffered::Queue::with_capacity(cap))
        };

        Queue {
            flavor: flavor,
            lock: Mutex::new(()),
            cond: Condvar::new(),
            blocked: AtomicUsize::new(0),
            senders: AtomicUsize::new(0),
            receivers: AtomicUsize::new(0),
        }
    }

    fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>
    ) -> Result<(), SendTimeoutError<T>> {
        match self.try_send(value) {
            Ok(()) => {},
            Err(TrySendError::Disconnected(v)) => {
                return Err(SendTimeoutError::Disconnected(v));
            }
            Err(TrySendError::Full(v)) => {
                value = v;

                loop {
                    let guard = self.lock.lock().unwrap();

                    if self.is_closed() {
                        return Err(SendTimeoutError::Disconnected(value));
                    }

                    self.blocked.fetch_add(1, SeqCst);

                    match self.try_send(value) {
                        Ok(()) => {
                            self.blocked.fetch_sub(1, SeqCst);
                            break;
                        }
                        Err(TrySendError::Disconnected(v)) => {
                            self.blocked.fetch_sub(1, SeqCst);
                            return Err(SendTimeoutError::Disconnected(v));
                        }
                        Err(TrySendError::Full(v)) => {
                            value = v;
                            match deadline {
                                None => {
                                    self.cond.wait(guard).unwrap();
                                }
                                Some(d) => {
                                    let now = Instant::now();
                                    if now >= d {
                                        return Err(SendTimeoutError::Timeout(value));
                                    }
                                    let (g, r) = self.cond.wait_timeout(guard, d - now).unwrap();
                                    if r.timed_out() {
                                        return Err(SendTimeoutError::Timeout(value));
                                    }
                                }
                            }
                            self.blocked.fetch_sub(1, SeqCst);
                        }
                    }
                }
            }
        }

        if self.blocked.load(SeqCst) > 0 {
            let _guard = self.lock.lock().unwrap();
            self.cond.notify_all();
        }

        Ok(())
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        if let Err(SendTimeoutError::Disconnected(v)) = self.send_until(value, None) {
            Err(SendError(v))
        } else {
            Ok(())
        }
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        self.send_until(value, Some(Instant::now() + dur))
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_send(value),
            Flavor::Buffered(ref q) => q.try_send(value),
            Flavor::Rendezvous(ref q) => q.try_send(value),
        }
    }

    fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let value;

        match self.try_recv() {
            Ok(v) => value = v,
            Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            Err(TryRecvError::Empty) => {
                loop {
                    let guard = self.lock.lock().unwrap();

                    if self.is_closed() {
                        return Err(RecvTimeoutError::Disconnected);
                    }

                    self.blocked.fetch_add(1, SeqCst); // TODO: can we move that into Err(Empty)?
                    // TODO: perhaps this should probably be a guard: see the last return Errs

                    match self.try_recv() {
                        Ok(v) => {
                            self.blocked.fetch_sub(1, SeqCst);
                            value = v;
                            break;
                        }
                        Err(TryRecvError::Disconnected) => {
                            self.blocked.fetch_sub(1, SeqCst);
                            return Err(RecvTimeoutError::Disconnected);
                        }
                        Err(TryRecvError::Empty) => {
                            match deadline {
                                None => {
                                    self.cond.wait(guard).unwrap();
                                }
                                Some(d) => {
                                    let now = Instant::now();
                                    if now >= d {
                                        return Err(RecvTimeoutError::Timeout);
                                    }
                                    let (g, r) = self.cond.wait_timeout(guard, d - now).unwrap();
                                    if r.timed_out() {
                                        return Err(RecvTimeoutError::Timeout);
                                    }
                                }
                            }
                            self.blocked.fetch_sub(1, SeqCst);
                        }
                    }
                }
            }
        };

        if self.blocked.load(SeqCst) > 0 {
            let _guard = self.lock.lock().unwrap();
            self.cond.notify_all();
        }

        Ok(value)
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        if let Ok(v) = self.recv_until(None) {
            Ok(v)
        } else {
            Err(RecvError)
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        self.recv_until(Some(Instant::now() + dur))
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_recv(),
            Flavor::Buffered(ref q) => q.try_recv(),
            Flavor::Rendezvous(ref q) => q.try_recv(),
        }
    }

    pub fn close(&self) -> bool {
        if self.is_closed() {
            return false;
        }
        println!("CLOSE");

        let _guard = self.lock.lock().unwrap();

        let result = match self.flavor {
            Flavor::Async(ref q) => q.close(),
            Flavor::Buffered(ref q) => q.close(),
            Flavor::Rendezvous(ref q) => q.close(),
        };

        if result && self.blocked.load(SeqCst) > 0 {
            self.cond.notify_all();
        }
        result
    }

    pub fn is_closed(&self) -> bool {
        match self.flavor {
            Flavor::Async(ref q) => q.is_closed(),
            Flavor::Buffered(ref q) => q.is_closed(),
            Flavor::Rendezvous(ref q) => q.is_closed(),
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

// struct Select<'a> {
//     v: Vec<&'a ()>,
// }
//
// impl<'a> Select<'a> {
//     fn new() -> Self {
//         Select {
//             v: vec![],
//         }
//     }
//
//     fn add<T>(&mut self, rx: &'a Receiver<T>) -> usize {
//         // self.v.push(r);
//         // self.f.push(Box::new(f));
//         self.v.len()
//     }
//
//     fn wait(&self) -> usize {
//         7
//     }
// }

fn foo() {
    let (t1, r1) = async::<i32>();
    let (t2, r2) = async::<i32>();

    // select! {
    //     a <- t1 {
    //
    //     }
    // }

    // let mut sel = Select::new();
    // let s1 = sel.recv(&r1);
    // let s2 = sel.send(&r2, 10);
    // let res = sel.wait();
    //
    // if res == s1 {
    //
    // } else if res == s2 {
    //
    // }
        // .add(&r1)
        // .add(&r2, |_| {x += 2;})
    //     .wait_timeout_ms(10);
        // .wait();
}
