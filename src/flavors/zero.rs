use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use CaseId;
use actor::{self, Actor, Packet};
use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};

struct Inner<T> {
    senders: Registry<T>,
    receivers: Registry<T>,
    closed: bool,
}

pub(crate) struct Channel<T> {
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

    pub fn promise_send(&self, case_id: CaseId) {
        self.inner.lock().senders.promise(case_id);
    }

    pub fn revoke_send(&self, case_id: CaseId) {
        self.inner.lock().senders.revoke(case_id);
    }

    pub unsafe fn fulfill_send(&self, value: T) {
        actor::current().finish_send(value) // TODO: current_finish_send
    }

    pub fn promise_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.promise(case_id);
    }

    pub fn revoke_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.revoke(case_id);
    }

    pub unsafe fn fulfill_recv(&self) -> T {
        actor::current().finish_recv() // TODO: current_finish_recv
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(TrySendError::Disconnected(value));
        }

        if let Some(e) = inner.receivers.pop() {
            drop(inner);
            match e.packet {
                None => {
                    e.actor.send(value);
                }
                Some(packet) => {
                    unsafe { (*packet).put(value) }
                    e.actor.unpark();
                }
            }
            Ok(())
        } else {
            Err(TrySendError::Full(value))
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(SendTimeoutError::Disconnected(value));
                }

                if let Some(e) = inner.receivers.pop() {
                    drop(inner);
                    match e.packet {
                        None => {
                            e.actor.send(value);
                        }
                        Some(packet) => {
                            unsafe { (*packet).put(value) }
                            e.actor.unpark();
                        }
                    }
                    return Ok(());
                }

                actor::current_reset();
                packet = Packet::new(Some(value));
                inner.senders.offer(&packet, case_id);
            }

            let timed_out = !actor::current_wait_until(deadline);

            if actor::current_selected() != CaseId::abort() {
                packet.wait();
                return Ok(());
            }

            let mut inner = self.inner.lock();
            inner.senders.revoke(case_id);
            value = packet.take();

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

        if let Some(e) = inner.senders.pop() {
            drop(inner);
            match e.packet {
                None => {
                    Ok(e.actor.recv())
                }
                Some(packet) => {
                    let v = unsafe { (*packet).take() };
                    // The actor has to lock `self.inner` before continuing.
                    e.actor.unpark();
                    Ok(v)
                }
            }
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        loop {
            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(RecvTimeoutError::Disconnected);
                }

                if let Some(e) = inner.senders.pop() {
                    drop(inner);
                    match e.packet {
                        None => {
                            return Ok(e.actor.recv())
                        }
                        Some(packet) => {
                            let v = unsafe { (*packet).take() };
                            // The actor has to lock `self.inner` before continuing.
                            e.actor.unpark();
                            return Ok(v)
                        }
                    }
                }

                actor::current_reset();
                packet = Packet::new(None);
                inner.receivers.offer(&packet, case_id);
            }

            let timed_out = !actor::current_wait_until(deadline);

            if actor::current_selected() != CaseId::abort() {
                packet.wait();
                return Ok(packet.take());
            }

            let mut inner = self.inner.lock();
            inner.receivers.revoke(case_id);

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
            inner.senders.abort_all();
            inner.receivers.abort_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }
}

pub struct Entry<T> {
    actor: Arc<Actor>,
    case_id: CaseId,
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
                if self.entries[i].actor.select(self.entries[i].case_id) {
                    return Some(self.entries.remove(i).unwrap());
                }
            }
        }

        None
    }

    fn offer(&mut self, packet: *const Packet<T>, case_id: CaseId) {
        self.entries.push_back(Entry {
            actor: actor::current(),
            case_id,
            packet: Some(packet),
        });
    }

    fn promise(&mut self, case_id: CaseId) {
        self.entries.push_back(Entry {
            actor: actor::current(),
            case_id,
            packet: None,
        });
    }

    fn revoke(&mut self, case_id: CaseId) {
        let thread_id = thread::current().id();

        if let Some((i, _)) = self.entries
            .iter()
            .enumerate()
            .find(|&(_, e)| e.actor.thread_id() == thread_id && e.case_id == case_id)
        {
            self.entries.remove(i);
            self.maybe_shrink();
        }
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

    fn abort_all(&mut self) {
        for e in self.entries.drain(..) {
            e.actor.select(CaseId::abort());
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

impl<T> Drop for Registry<T> {
    fn drop(&mut self) {
        debug_assert!(self.entries.is_empty());
    }
}
