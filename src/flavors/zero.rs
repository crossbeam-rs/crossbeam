use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use CaseId;
use actor::{self, Actor};
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

    pub fn fulfill_send(&self, value: T) {
        finish_send(value, self) // TODO: current_finish_send
    }

    pub fn promise_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.promise(case_id);
    }

    pub fn revoke_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.revoke(case_id);
    }

    pub fn fulfill_recv(&self) -> T {
        finish_recv(self) // TODO: current_finish_recv
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
                    send(&e.local, value, self);
                }
                Some(packet) => {
                    unsafe { (*packet).put(value) }
                    e.local.actor.unpark();
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
                            send(&e.local, value, self);
                        }
                        Some(packet) => {
                            unsafe { (*packet).put(value) }
                            e.local.actor.unpark();
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
                None => Ok(recv(&e.local, self)),
                Some(packet) => {
                    let v = unsafe { (*packet).take() };
                    // The actor has to lock `self.inner` before continuing.
                    e.local.actor.unpark();
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
                        None => return Ok(recv(&e.local, self)),
                        Some(packet) => {
                            let v = unsafe { (*packet).take() };
                            // The actor has to lock `self.inner` before continuing.
                            e.local.actor.unpark();
                            return Ok(v);
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
    local: Arc<Local>,
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
            if self.entries[i].local.actor.thread_id() != thread_id {
                if self.entries[i].local.actor.select(self.entries[i].case_id) {
                    return Some(self.entries.remove(i).unwrap());
                }
            }
        }

        None
    }

    fn offer(&mut self, packet: *const Packet<T>, case_id: CaseId) {
        self.entries.push_back(Entry {
            local: LOCAL.with(|l| l.clone()),
            case_id,
            packet: Some(packet),
        });
    }

    fn promise(&mut self, case_id: CaseId) {
        self.entries.push_back(Entry {
            local: LOCAL.with(|l| l.clone()),
            case_id,
            packet: None,
        });
    }

    fn revoke(&mut self, case_id: CaseId) {
        let thread_id = thread::current().id();

        if let Some((i, _)) = self.entries.iter().enumerate().find(|&(_, e)| {
            e.case_id == case_id && e.local.actor.thread_id() == thread_id
        }) {
            self.entries.remove(i);
            self.maybe_shrink();
        }
    }

    fn can_notify(&self) -> bool {
        let thread_id = thread::current().id();

        for i in 0..self.entries.len() {
            if self.entries[i].local.actor.thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    fn abort_all(&mut self) {
        for e in self.entries.drain(..) {
            e.local.actor.select(CaseId::abort());
            e.local.actor.unpark();
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

thread_local! {
    static LOCAL: Arc<Local> = Arc::new(Local {
        actor: actor::current(),
        request_ptr: AtomicUsize::new(0),
    });
}

struct Local {
    actor: Arc<Actor>,
    request_ptr: AtomicUsize,
}

fn finish_send<T>(value: T, chan: &Channel<T>) {
    let req = loop {
        let ptr = LOCAL.with(|l| l.request_ptr.swap(0, SeqCst) as *const Request<T>);
        if !ptr.is_null() {
            break ptr;
        }
        thread::yield_now();
    };

    unsafe {
        assert!((*req).chan == chan);

        let actor = (*req).actor.clone();
        (*req).packet.put(value);
        (*req).actor.select(CaseId::abort());
        actor.unpark();
    }
}

fn finish_recv<T>(chan: &Channel<T>) -> T {
    let req = loop {
        let ptr = LOCAL.with(|l| l.request_ptr.swap(0, SeqCst) as *const Request<T>);
        if !ptr.is_null() {
            break ptr;
        }
        thread::yield_now();
    };

    unsafe {
        assert!((*req).chan == chan);

        let actor = (*req).actor.clone();
        let v = (*req).packet.take();
        (*req).actor.select(CaseId::abort());
        actor.unpark();
        v
    }
}

fn recv<T>(other: &Local, chan: &Channel<T>) -> T {
    actor::current_reset();
    let req = Request::new(None, chan);
    other.request_ptr.store(&req as *const _ as usize, SeqCst);
    other.actor.unpark();
    actor::current_wait_until(None);
    req.packet.take()
}

fn send<T>(other: &Local, value: T, chan: &Channel<T>) {
    actor::current_reset();
    let req = Request::new(Some(value), chan);
    other.request_ptr.store(&req as *const _ as usize, SeqCst);
    other.actor.unpark();
    actor::current_wait_until(None);
}

struct Request<T> {
    actor: Arc<Actor>,
    packet: Packet<T>,
    chan: *const Channel<T>,
}

impl<T> Request<T> {
    fn new(data: Option<T>, chan: &Channel<T>) -> Self {
        Request {
            actor: actor::current(),
            packet: Packet::new(data),
            chan,
        }
    }
}

struct Packet<T>(Mutex<Option<T>>, AtomicBool);

impl<T> Packet<T> {
    fn new(data: Option<T>) -> Self {
        Packet(Mutex::new(data), AtomicBool::new(false))
    }

    fn put(&self, data: T) {
        {
            let mut opt = self.0.try_lock().unwrap();
            assert!(opt.is_none());
            *opt = Some(data);
        }
        self.1.store(true, SeqCst);
    }

    fn take(&self) -> T {
        let t = self.0.try_lock().unwrap().take().unwrap();
        self.1.store(true, SeqCst);
        t
    }

    fn wait(&self) {
        for i in 0..10 {
            if self.1.load(SeqCst) {
                return;
            }
            for _ in 0..1 << i {
                // ::std::sync::atomic::hint_core_should_pause();
            }
        }

        loop {
            if self.1.load(SeqCst) {
                return;
            }
            thread::yield_now();
        }
    }
}
