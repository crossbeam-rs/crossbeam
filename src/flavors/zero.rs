use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use actor::{self, Actor, HandleId, Packet, Request};

struct Inner<T> {
    senders: Registry<T>,
    receivers: Registry<T>,
    closed: bool,
}

pub struct Channel<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        Channel {
            inner: Mutex::new(Inner {
                senders: Registry::new(),
                receivers: Registry::new(),
                closed: false,
            }),
        }
    }

    pub fn promise_send(&self, id: HandleId) {
        self.inner.lock().senders.promise(id);
    }

    pub fn revoke_send(&self, id: HandleId) {
        self.inner.lock().senders.revoke(id);
    }

    pub fn promise_recv(&self, id: HandleId) {
        self.inner.lock().receivers.promise(id);
    }

    pub fn revoke_recv(&self, id: HandleId) {
        self.inner.lock().receivers.revoke(id);
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(TrySendError::Disconnected(value));
        }

        while let Some(e) = inner.receivers.pop() {
            match e.packet {
                None => if e.actor.select(e.id) {
                    let req = Request::new(Some(value));
                    actor::current().reset();
                    e.actor.set_request(&req);
                    drop(inner);

                    e.actor.unpark();
                    actor::current().wait_until(None);
                    return Ok(());
                },
                Some(packet) => if e.actor.select(e.id) {
                    unsafe { (*packet).put(value) }
                    e.actor.unpark();
                    return Ok(());
                },
            }
        }

        Err(TrySendError::Full(value))
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            match self.try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(v)) => value = v,
                Err(TrySendError::Disconnected(v)) => return Err(SendTimeoutError::Disconnected(v)),
            }

            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(SendTimeoutError::Disconnected(value));
                }

                if inner.receivers.can_notify() {
                    continue;
                }

                actor::current().reset();
                packet = Packet::new(Some(value));
                inner.senders.offer(&packet, HandleId::sentinel());
            }

            let timed_out = !actor::current().wait_until(deadline);
            let mut inner = self.inner.lock();
            inner.senders.revoke(HandleId::sentinel());

            match packet.take() {
                None => return Ok(()),
                Some(v) => value = v,
            }

            if timed_out {
                return Err(SendTimeoutError::Timeout(value));
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(TryRecvError::Disconnected);
        }

        while let Some(e) = inner.senders.pop() {
            match e.packet {
                None => if e.actor.select(e.id) {
                    let req = Request::new(None);
                    actor::current().reset();
                    e.actor.set_request(&req);
                    drop(inner);

                    e.actor.unpark();
                    actor::current().wait_until(None);
                    let _inner = self.inner.lock();
                    let v = req.packet.take().unwrap();
                    return Ok(v);
                },
                Some(packet) => if e.actor.select(e.id) {
                    let v = unsafe { (*packet).take().unwrap() };
                    e.actor.unpark();
                    return Ok(v);
                },
            }
        }

        Err(TryRecvError::Empty)
    }

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            }

            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(RecvTimeoutError::Disconnected);
                }

                if inner.senders.can_notify() {
                    continue;
                }

                actor::current().reset();
                packet = Packet::new(None);
                inner.receivers.offer(&packet, HandleId::sentinel());
            }

            let timed_out = !actor::current().wait_until(deadline);
            let mut inner = self.inner.lock();
            inner.receivers.revoke(HandleId::sentinel());

            if let Some(v) = packet.take() {
                return Ok(v);
            }

            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    pub fn can_recv(&self) -> bool {
        self.inner.lock().senders.can_notify()
    }

    pub fn can_send(&self) -> bool {
        self.inner.lock().receivers.can_notify()
    }

    pub fn close(&self) -> bool {
        let mut inner = self.inner.lock();

        if inner.closed {
            false
        } else {
            inner.closed = true;
            inner.senders.notify_all();
            inner.receivers.notify_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }
}

pub struct Entry<T> {
    actor: Arc<Actor>,
    id: HandleId,
    packet: Option<*const Packet<T>>,
}

struct Registry<T> {
    entries: VecDeque<Entry<T>>,
}

impl<T> Registry<T> {
    fn new() -> Self {
        Registry {
            entries: VecDeque::new(),
        }
    }

    fn pop(&mut self) -> Option<Entry<T>> {
        let thread_id = thread::current().id();

        for i in 0..self.entries.len() {
            if self.entries[i].actor.thread_id() != thread_id {
                let e = self.entries.remove(i).unwrap();
                return Some(e);
            }
        }

        None
    }

    fn offer(&mut self, packet: *const Packet<T>, id: HandleId) {
        self.entries.push_back(Entry {
            actor: actor::current(),
            id,
            packet: Some(packet),
        });
    }

    fn promise(&mut self, id: HandleId) {
        self.entries.push_back(Entry {
            actor: actor::current(),
            id,
            packet: None,
        });
    }

    fn revoke(&mut self, id: HandleId) {
        let thread_id = thread::current().id();
        self.entries
            .retain(|e| e.actor.thread_id() != thread_id || e.id != id);
        self.maybe_shrink();
    }

    fn can_notify(&self) -> bool {
        let thread_id = thread::current().id();

        for i in 0..self.entries.len() {
            if self.entries[i].actor.thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    fn notify_all(&mut self) {
        for e in self.entries.drain(..) {
            e.actor.select(e.id);
            e.actor.unpark();
        }
        self.maybe_shrink();
    }

    fn maybe_shrink(&mut self) {
        if self.entries.capacity() > 32 && self.entries.capacity() / 2 > self.entries.len() {
            self.entries.shrink_to_fit();
        }
    }
}
