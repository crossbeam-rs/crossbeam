extern crate coco;
extern crate either;

use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};

mod async;
pub mod buffered;
// mod rendezvous;

// TODO: len() (add counters to each Node in the async version)
// TODO: is_empty()
// TODO: make Sender and Receiver Send + Sync

pub struct SendError<T>(pub T);

pub enum TrySendError<T> {
    Full(T),
    Closed(T),
}

pub struct RecvError;

pub enum TryRecvError {
    Empty,
    Closed,
}

enum Flavor<T> {
    Async(async::Queue<T>),
    Buffered(buffered::Queue<T>),
    Rendezvous(),
}

pub struct Queue<T> {
    flavor: Flavor<T>,
    lock: Mutex<()>,
    cond: Condvar,
    blocked: AtomicUsize,
}

impl<T> Queue<T> {
    pub fn async() -> Self {
        Queue {
            flavor: Flavor::Async(async::Queue::new()),
            lock: Mutex::new(()),
            cond: Condvar::new(),
            blocked: AtomicUsize::new(0),
        }
    }

    pub fn sync(cap: usize) -> Self {
        let flavor = if cap == 0 {
            Flavor::Rendezvous()//(unimplemented!())
        } else {
            Flavor::Buffered(buffered::Queue::with_capacity(cap))
        };

        Queue {
            flavor: flavor,
            lock: Mutex::new(()),
            cond: Condvar::new(),
            blocked: AtomicUsize::new(0),
        }
    }

    pub fn send(&self, mut value: T) -> Result<(), SendError<T>> {
        match self.try_send(value) {
            Ok(()) => {},
            Err(TrySendError::Closed(v)) => return Err(SendError(v)),
            Err(TrySendError::Full(v)) => {
                value = v;

                loop {
                    let guard = self.lock.lock().unwrap();

                    if self.is_closed() {
                        return Err(SendError(value));
                    }

                    self.blocked.fetch_add(1, SeqCst);

                    match self.try_send(value) {
                        Ok(()) => {
                            self.blocked.fetch_sub(1, SeqCst);
                            break;
                        }
                        Err(TrySendError::Closed(v)) => {
                            self.blocked.fetch_sub(1, SeqCst);
                            return Err(SendError(v));
                        }
                        Err(TrySendError::Full(v)) => {
                            value = v;
                            self.cond.wait(guard).unwrap();
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

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_send(value),
            Flavor::Buffered(ref q) => q.try_send(value),
            Flavor::Rendezvous() => unimplemented!(),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        let result = self.try_recv().or_else(|_| {
            loop {
                let guard = self.lock.lock().unwrap();

                if self.is_closed() {
                    return Err(RecvError);
                }

                self.blocked.fetch_add(1, SeqCst);

                match self.try_recv() {
                    Ok(v) => {
                        self.blocked.fetch_sub(1, SeqCst);
                        return Ok(v);
                    }
                    Err(TryRecvError::Closed) => {
                        self.blocked.fetch_sub(1, SeqCst);
                        return Err(RecvError);
                    }
                    Err(TryRecvError::Empty) => {
                        self.cond.wait(guard).unwrap();
                        self.blocked.fetch_sub(1, SeqCst);
                    }
                }
            }
        });

        if self.blocked.load(SeqCst) > 0 {
            let _guard = self.lock.lock().unwrap();
            self.cond.notify_all();
        }

        result
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.flavor {
            Flavor::Async(ref q) => q.try_recv(),
            Flavor::Buffered(ref q) => q.try_recv(),
            Flavor::Rendezvous() => unimplemented!(),
        }
    }

    pub fn close(&self) -> bool {
        let _guard = self.lock.lock().unwrap();

        let result = match self.flavor {
            Flavor::Async(ref q) => q.close(),
            Flavor::Buffered(ref q) => q.close(),
            Flavor::Rendezvous() => unimplemented!(),
        };

        self.cond.notify_all();
        result
    }

    pub fn is_closed(&self) -> bool {
        match self.flavor {
            Flavor::Async(ref q) => q.is_closed(),
            Flavor::Buffered(ref q) => q.is_closed(),
            Flavor::Rendezvous() => unimplemented!(),
        }
    }
}

pub struct Sender<T>(Arc<Queue<T>>);

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        unimplemented!()
    }

    pub fn try_send(&self, t: T) -> Result<(), TrySendError<T>> {
        unimplemented!()
    }
}

pub struct Receiver<T>(Arc<Queue<T>>);

impl<T> Receiver<T> {
    pub fn recv(&self) -> Result<T, RecvError> {
        unimplemented!()
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        unimplemented!()
    }
}

pub fn async<T>() -> (Sender<T>, Receiver<T>) {
    unimplemented!()
}

pub fn sync<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    unimplemented!()
}
