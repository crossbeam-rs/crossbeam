use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use flavors;
use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use select::CaseId;

pub struct Channel<T> {
    senders: AtomicUsize,
    receivers: AtomicUsize,
    flavor: Flavor<T>,
}

enum Flavor<T> {
    Array(flavors::array::Channel<T>),
    List(flavors::list::Channel<T>),
    Zero(flavors::zero::Channel<T>),
}

pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        flavor: Flavor::List(flavors::list::Channel::new()),
    });
    (Sender::new(chan.clone()), Receiver::new(chan))
}

pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        flavor: {
            if cap == 0 {
                Flavor::Zero(flavors::zero::Channel::new())
            } else {
                Flavor::Array(flavors::array::Channel::with_capacity(cap))
            }
        },
    });
    (Sender::new(chan.clone()), Receiver::new(chan))
}

pub struct Sender<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> Sender<T> {
    fn new(chan: Arc<Channel<T>>) -> Self {
        chan.senders.fetch_add(1, SeqCst);
        Sender(chan)
    }

    pub(crate) fn case_id(&self) -> CaseId {
        CaseId::new(self as *const _ as usize)
    }

    pub(crate) fn promise_send(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().register(self.case_id()),
            Flavor::List(_) => {}
            Flavor::Zero(ref chan) => chan.promise_send(self.case_id()),
        }
    }

    pub(crate) fn revoke_send(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().unregister(self.case_id()),
            Flavor::List(_) => {}
            Flavor::Zero(ref chan) => chan.revoke_send(self.case_id()),
        }
    }

    pub(crate) fn can_send(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => !chan.is_full(),
            Flavor::List(_) => true,
            Flavor::Zero(ref chan) => chan.can_send(),
        }
    }

    pub(crate) fn fulfill_send(&self, value: T) -> Result<(), T> {
        match self.0.flavor {
            Flavor::Array(_) | Flavor::List(_) => match self.try_send(value) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(v)) => Err(v),
                Err(TrySendError::Disconnected(v)) => Err(v),
            },
            Flavor::Zero(ref chan) => {
                chan.fulfill_send(value);
                Ok(())
            }
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.try_send(value),
            Flavor::List(ref chan) => chan.try_send(value),
            Flavor::Zero(ref chan) => chan.try_send(value, self.case_id()),
        }
    }

    // TODO: explain that a zero-capacity channel is always full and empty.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let res = match self.0.flavor {
            Flavor::Array(ref chan) => chan.send_until(value, None, self.case_id()),
            Flavor::List(ref chan) => chan.send(value),
            Flavor::Zero(ref chan) => chan.send_until(value, None, self.case_id()),
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
            Flavor::Array(ref chan) => chan.send_until(value, deadline, self.case_id()),
            Flavor::List(ref chan) => chan.send(value),
            Flavor::Zero(ref chan) => chan.send_until(value, deadline, self.case_id()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.len(),
            Flavor::List(ref chan) => chan.len(),
            Flavor::Zero(_) => 0,
        }
    }

    pub fn capacity(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => Some(chan.capacity()),
            Flavor::List(_) => None,
            Flavor::Zero(_) => Some(0),
        }
    }

    pub fn is_disconnected(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_closed(),
            Flavor::List(ref chan) => chan.is_closed(),
            Flavor::Zero(ref chan) => chan.is_closed(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, SeqCst) == 1 {
            match self.0.flavor {
                Flavor::Array(ref chan) => chan.close(),
                Flavor::List(ref chan) => chan.close(),
                Flavor::Zero(ref chan) => chan.close(),
            };
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender::new(self.0.clone())
    }
}

pub struct Receiver<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> Receiver<T> {
    fn new(chan: Arc<Channel<T>>) -> Self {
        chan.receivers.fetch_add(1, SeqCst);
        Receiver(chan)
    }

    pub(crate) fn case_id(&self) -> CaseId {
        CaseId::new(self as *const _ as usize)
    }

    pub(crate) fn promise_recv(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.receivers().register(self.case_id()),
            Flavor::List(ref chan) => chan.receivers().register(self.case_id()),
            Flavor::Zero(ref chan) => chan.promise_recv(self.case_id()),
        }
    }

    pub(crate) fn revoke_recv(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.receivers().unregister(self.case_id()),
            Flavor::List(ref chan) => chan.receivers().unregister(self.case_id()),
            Flavor::Zero(ref chan) => chan.revoke_recv(self.case_id()),
        }
    }

    pub(crate) fn can_recv(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => !chan.is_empty(),
            Flavor::List(ref chan) => !chan.is_empty(),
            Flavor::Zero(ref chan) => chan.can_recv(),
        }
    }

    pub(crate) fn fulfill_recv(&self) -> Result<T, ()> {
        match self.0.flavor {
            Flavor::Array(_) | Flavor::List(_) => self.try_recv().map_err(|_| ()),
            Flavor::Zero(ref chan) => Ok(chan.fulfill_recv()),
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.try_recv(),
            Flavor::List(ref chan) => chan.try_recv(),
            Flavor::Zero(ref chan) => chan.try_recv(self.case_id()),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        let res = match self.0.flavor {
            Flavor::Array(ref chan) => chan.recv_until(None, self.case_id()),
            Flavor::List(ref chan) => chan.recv_until(None, self.case_id()),
            Flavor::Zero(ref chan) => chan.recv_until(None, self.case_id()),
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
            Flavor::Array(ref chan) => chan.recv_until(deadline, self.case_id()),
            Flavor::List(ref chan) => chan.recv_until(deadline, self.case_id()),
            Flavor::Zero(ref chan) => chan.recv_until(deadline, self.case_id()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.len(),
            Flavor::List(ref chan) => chan.len(),
            Flavor::Zero(_) => 0,
        }
    }

    pub fn capacity(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => Some(chan.capacity()),
            Flavor::List(_) => None,
            Flavor::Zero(_) => Some(0),
        }
    }

    pub fn is_disconnected(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_closed(),
            Flavor::List(ref chan) => chan.is_closed(),
            Flavor::Zero(ref chan) => chan.is_closed(),
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
                Flavor::Array(ref chan) => chan.close(),
                Flavor::List(ref chan) => chan.close(),
                Flavor::Zero(ref chan) => chan.close(),
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
