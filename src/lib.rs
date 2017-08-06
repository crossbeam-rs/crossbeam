#![feature(hint_core_should_pause)]

extern crate coco;
extern crate crossbeam;
extern crate parking_lot;
extern crate rand;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
pub use select::Select;

mod actor;
mod err;
mod flavors;
mod watch;
mod select;

// TODO: Use CachePadded

// TODO: The IsReady check must also check for closing (same in *_until methods)
// TODO: Panic if two selects are running at the same time
// TODO: select with recv & send on the same channel (all flavors) should work. Perhaps notify_one() must skip the current thread
// TODO: `default` case in Select (like in go and chan crate)

#[derive(Clone, Copy)]
struct Backoff(usize);

// TODO: Move into a separate module
impl Backoff {
    #[inline]
    fn new() -> Self {
        Backoff(0)
    }

    #[inline]
    fn abort(&mut self) {
        self.0 = !0;
    }

    #[inline]
    fn tick(&mut self) -> bool {
        if self.0 >= 20 {
            false
        } else {
            if self.0 <= 10 {
                for _ in 0..1 << self.0 {
                    ::std::sync::atomic::hint_core_should_pause();
                }
            } else {
                thread::yield_now();
            }

            self.0 += 1;
            true
        }
    }
}

enum Flavor<T> {
    Array(flavors::array::Queue<T>),
    List(flavors::list::Queue<T>),
    Zero(flavors::zero::Queue<T>),
}

struct Queue<T> {
    senders: AtomicUsize,
    receivers: AtomicUsize,
    flavor: Flavor<T>,
}

pub struct Sender<T>(Arc<Queue<T>>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> Sender<T> {
    fn new(q: Arc<Queue<T>>) -> Self {
        q.senders.fetch_add(1, SeqCst);
        Sender(q)
    }

    pub(crate) fn id(&self) -> usize {
        let addr = match self.0.flavor {
            Flavor::Array(ref q) => q as *const flavors::array::Queue<T> as usize,
            Flavor::List(ref q) => q as *const flavors::list::Queue<T> as usize,
            Flavor::Zero(ref q) => q as *const flavors::zero::Queue<T> as usize,
        };
        assert!(addr % 2 == 0);
        addr
    }

    pub(crate) fn can_send(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref q) => q.len() < q.capacity(),
            Flavor::List(ref q) => false,
            Flavor::Zero(ref q) => q.can_send(),
        }
    }

    pub(crate) fn try_send_with_backoff(
        &self,
        value: T,
        backoff: &mut Backoff,
    ) -> Result<(), TrySendError<T>> {
        match self.0.flavor {
            Flavor::Array(ref q) => q.try_send_with_backoff(value, backoff),
            Flavor::List(ref q) => q.try_send(value),
            Flavor::Zero(ref q) => {
                backoff.abort();
                q.try_send(value)
            }
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        self.try_send_with_backoff(value, &mut Backoff::new())
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let res = match self.0.flavor {
            Flavor::Array(ref q) => q.send_until(value, None),
            Flavor::List(ref q) => q.send_until(value, None),
            Flavor::Zero(ref q) => q.send_until(value, None),
        };
        match res {
            Ok(()) => Ok(()),
            Err(SendTimeoutError::Disconnected(v)) => Err(SendError(v)),
            Err(SendTimeoutError::Timeout(v)) => Err(SendError(v)),
        }
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        let deadline = Some(Instant::now() + dur);
        match self.0.flavor {
            Flavor::Array(ref q) => q.send_until(value, deadline),
            Flavor::List(ref q) => q.send_until(value, deadline),
            Flavor::Zero(ref q) => q.send_until(value, deadline),
        }
    }

    pub fn len(&self) -> usize {
        match self.0.flavor {
            Flavor::Array(ref q) => q.len(),
            Flavor::List(ref q) => q.len(),
            Flavor::Zero(ref q) => 0,
        }
    }

    // `true` if `try_send` would fail with `TrySendErr::Full(_)`
    pub fn is_full(&self) -> bool {
        !self.can_send()
    }

    pub fn is_disconnected(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref q) => q.is_closed(),
            Flavor::List(ref q) => q.is_closed(),
            Flavor::Zero(ref q) => q.is_closed(),
        }
    }

    pub fn capacity(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref q) => Some(q.capacity()),
            Flavor::List(ref q) => None,
            Flavor::Zero(ref q) => Some(0),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, SeqCst) == 1 {
            match self.0.flavor {
                Flavor::Array(ref q) => q.close(),
                Flavor::List(ref q) => q.close(),
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

pub struct Receiver<T>(Arc<Queue<T>>);

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> Receiver<T> {
    fn new(q: Arc<Queue<T>>) -> Self {
        q.receivers.fetch_add(1, SeqCst);
        Receiver(q)
    }

    pub(crate) fn id(&self) -> usize {
        let addr = match self.0.flavor {
            Flavor::Array(ref q) => q as *const flavors::array::Queue<T> as usize,
            Flavor::List(ref q) => q as *const flavors::list::Queue<T> as usize,
            Flavor::Zero(ref q) => q as *const flavors::zero::Queue<T> as usize,
        };
        assert!(addr % 2 == 0);
        addr | 1
    }

    pub(crate) fn can_recv(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref q) => q.len() > 0,
            Flavor::List(ref q) => q.len() > 0,
            Flavor::Zero(ref q) => q.can_recv(),
        }
    }

    pub(crate) fn try_recv_with_backoff(&self, backoff: &mut Backoff) -> Result<T, TryRecvError> {
        match self.0.flavor {
            Flavor::Array(ref q) => q.try_recv_with_backoff(backoff),
            Flavor::List(ref q) => q.try_recv_with_backoff(backoff),
            Flavor::Zero(ref q) => {
                backoff.abort();
                q.try_recv()
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.try_recv_with_backoff(&mut Backoff::new())
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        let res = match self.0.flavor {
            Flavor::Array(ref q) => q.recv_until(None),
            Flavor::List(ref q) => q.recv_until(None),
            Flavor::Zero(ref q) => q.recv_until(None),
        };
        if let Ok(v) = res {
            Ok(v)
        } else {
            Err(RecvError)
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        let deadline = Some(Instant::now() + dur);
        match self.0.flavor {
            Flavor::Array(ref q) => q.recv_until(deadline),
            Flavor::List(ref q) => q.recv_until(deadline),
            Flavor::Zero(ref q) => q.recv_until(deadline),
        }
    }

    pub fn len(&self) -> usize {
        match self.0.flavor {
            Flavor::Array(ref q) => q.len(),
            Flavor::List(ref q) => q.len(),
            Flavor::Zero(ref q) => 0,
        }
    }

    // `true` if `try_recv` would fail with `TryRecvError::Empty`
    pub fn is_empty(&self) -> bool {
        !self.can_recv()
    }

    pub fn is_disconnected(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref q) => q.is_closed(),
            Flavor::List(ref q) => q.is_closed(),
            Flavor::Zero(ref q) => q.is_closed(),
        }
    }

    pub fn capacity(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref q) => Some(q.capacity()),
            Flavor::List(_) => None,
            Flavor::Zero(_) => Some(0),
        }
    }

    pub fn iter(&self) -> Iter<T> {
        Iter { rx: self }
    }

    pub fn try_iter(&self) -> TryIter<T> {
        TryIter { rx: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if self.0.receivers.fetch_sub(1, SeqCst) == 1 {
            match self.0.flavor {
                Flavor::Array(ref q) => q.close(),
                Flavor::List(ref q) => q.close(),
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

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { rx: self }
    }
}

pub struct Iter<'a, T: 'a> {
    rx: &'a Receiver<T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}

pub struct TryIter<'a, T: 'a> {
    rx: &'a Receiver<T>,
}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.try_recv().ok()
    }
}

pub struct IntoIter<T> {
    rx: Receiver<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}

pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let q = Arc::new(Queue {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        flavor: Flavor::List(flavors::list::Queue::new()),
    });
    (Sender::new(q.clone()), Receiver::new(q))
}

pub fn bounded<T>(size: usize) -> (Sender<T>, Receiver<T>) {
    let q = Arc::new(Queue {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        flavor: if size == 0 {
            Flavor::Zero(flavors::zero::Queue::new())
        } else {
            Flavor::Array(flavors::array::Queue::with_capacity(size))
        },
    });
    (Sender::new(q.clone()), Receiver::new(q))
}
