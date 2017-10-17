use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use select::CaseId;
use select::handle::{self, Handle};
use util::Backoff;

enum Operation {
    Try,
    Spin,
    Until(Option<Instant>),
}

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

    pub fn promise_send(&self, case_id: CaseId) {
        self.inner.lock().senders.promise(case_id);
    }

    pub fn revoke_send(&self, case_id: CaseId) {
        self.inner.lock().senders.revoke(case_id);
    }

    pub fn fulfill_send(&self, value: T) {
        finish_send(value, self)
    }

    pub fn promise_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.promise(case_id);
    }

    pub fn revoke_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.revoke(case_id);
    }

    pub fn fulfill_recv(&self) -> T {
        finish_recv(self)
    }

    pub fn try_send(&self, value: T, case_id: CaseId) -> Result<(), TrySendError<T>> {
        match self.send(value, Operation::Try, case_id) {
            Ok(()) => Ok(()),
            Err(SendTimeoutError::Disconnected(v)) => Err(TrySendError::Disconnected(v)),
            Err(SendTimeoutError::Timeout(v)) => Err(TrySendError::Full(v)),
        }
    }

    pub fn spin_try_send(&self, value: T, case_id: CaseId) -> Result<(), TrySendError<T>> {
        match self.send(value, Operation::Spin, case_id) {
            Ok(()) => Ok(()),
            Err(SendTimeoutError::Disconnected(v)) => Err(TrySendError::Disconnected(v)),
            Err(SendTimeoutError::Timeout(v)) => Err(TrySendError::Full(v)),
        }
    }

    pub fn send_until(
        &self,
        value: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<(), SendTimeoutError<T>> {
        self.send(value, Operation::Until(deadline), case_id)
    }

    fn send(
        &self,
        mut value: T,
        oper: Operation,
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
                    e.send(value, self);
                    return Ok(());
                }

                if let Operation::Try = oper {
                    return Err(SendTimeoutError::Timeout(value));
                }

                handle::current_reset();
                packet = Packet::new(Some(value));
                inner.senders.offer(&packet, case_id);
            }

            let timed_out = if let Operation::Until(deadline) = oper {
                !handle::current_wait_until(deadline)
            } else {
                thread::yield_now();
                handle::current_select(CaseId::abort())
            };

            if handle::current_selected() != CaseId::abort() {
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

    pub fn try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.recv(Operation::Try, case_id) {
            Ok(v) => Ok(v),
            Err(RecvTimeoutError::Disconnected) => Err(TryRecvError::Disconnected),
            Err(RecvTimeoutError::Timeout) => Err(TryRecvError::Empty),
        }
    }

    pub fn spin_try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.recv(Operation::Spin, case_id) {
            Ok(v) => Ok(v),
            Err(RecvTimeoutError::Disconnected) => Err(TryRecvError::Disconnected),
            Err(RecvTimeoutError::Timeout) => Err(TryRecvError::Empty),
        }
    }

    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        self.recv(Operation::Until(deadline), case_id)
    }

    fn recv(
        &self,
        oper: Operation,
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
                    return Ok(e.recv(self));
                }

                if let Operation::Try = oper {
                    return Err(RecvTimeoutError::Timeout);
                }

                handle::current_reset();
                packet = Packet::new(None);
                inner.receivers.offer(&packet, case_id);
            }

            let timed_out = if let Operation::Until(deadline) = oper {
                !handle::current_wait_until(deadline)
            } else {
                thread::yield_now();
                handle::current_select(CaseId::abort())
            };

            if handle::current_selected() != CaseId::abort() {
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

enum Entry<T> {
    Offer {
        handle: Handle,
        packet: *const Packet<T>,
        case_id: CaseId,
    },
    Promise { local: Arc<Local>, case_id: CaseId },
}

impl<T> Entry<T> {
    fn send(&self, value: T, chan: &Channel<T>) {
        match *self {
            Entry::Offer {
                ref handle, packet, ..
            } => {
                unsafe { (*packet).put(value) }
                handle.unpark();
            }
            Entry::Promise { ref local, .. } => {
                handle::current_reset();
                let req = Request::new(Some(value), chan);
                local.request_ptr.store(&req as *const _ as usize, SeqCst);
                local.handle.unpark();
                handle::current_wait_until(None);
            }
        }
    }

    fn recv(&self, chan: &Channel<T>) -> T {
        match *self {
            Entry::Offer {
                ref handle, packet, ..
            } => {
                let v = unsafe { (*packet).take() };
                // The handle has to lock `self.inner` before continuing.
                handle.unpark();
                v
            }
            Entry::Promise { ref local, .. } => {
                handle::current_reset();
                let req = Request::new(None, chan);
                local.request_ptr.store(&req as *const _ as usize, SeqCst);
                local.handle.unpark();
                handle::current_wait_until(None);
                req.packet.take()
            }
        }
    }

    fn handle(&self) -> &Handle {
        match *self {
            Entry::Offer { ref handle, .. } => handle,
            Entry::Promise { ref local, .. } => &local.handle,
        }
    }

    fn case_id(&self) -> CaseId {
        match *self {
            Entry::Offer { case_id, .. } => case_id,
            Entry::Promise { case_id, .. } => case_id,
        }
    }
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

        let handle = (*req).handle.clone();
        (*req).packet.put(value);
        (*req).handle.select(CaseId::abort());
        handle.unpark();
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

        let handle = (*req).handle.clone();
        let v = (*req).packet.take();
        (*req).handle.select(CaseId::abort());
        handle.unpark();
        v
    }
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
        let thread_id = current_thread_id();

        for i in 0..self.entries.len() {
            if self.entries[i].handle().thread_id() != thread_id {
                if self.entries[i].handle().select(self.entries[i].case_id()) {
                    return Some(self.entries.remove(i).unwrap());
                }
            }
        }

        None
    }

    fn offer(&mut self, packet: *const Packet<T>, case_id: CaseId) {
        self.entries.push_back(Entry::Offer {
            handle: handle::current(),
            packet,
            case_id,
        });
    }

    fn promise(&mut self, case_id: CaseId) {
        self.entries.push_back(Entry::Promise {
            local: LOCAL.with(|l| l.clone()),
            case_id,
        });
    }

    fn revoke(&mut self, case_id: CaseId) {
        let thread_id = current_thread_id();

        if let Some((i, _)) = self.entries.iter().enumerate().find(|&(_, e)| {
            e.case_id() == case_id && e.handle().thread_id() == thread_id
        }) {
            self.entries.remove(i);
            self.maybe_shrink();
        }
    }

    fn can_notify(&self) -> bool {
        let thread_id = current_thread_id();

        for i in 0..self.entries.len() {
            if self.entries[i].handle().thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    fn abort_all(&mut self) {
        for e in self.entries.drain(..) {
            e.handle().select(CaseId::abort());
            e.handle().unpark();
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
    static THREAD_ID: thread::ThreadId = thread::current().id();
}

fn current_thread_id() -> thread::ThreadId {
    THREAD_ID.with(|id| *id)
}

thread_local! {
    static LOCAL: Arc<Local> = Arc::new(Local {
        handle: handle::current(),
        request_ptr: AtomicUsize::new(0),
    });
}

struct Local {
    handle: Handle,
    request_ptr: AtomicUsize,
}

struct Request<T> {
    handle: Handle,
    packet: Packet<T>,
    chan: *const Channel<T>,
}

impl<T> Request<T> {
    fn new(data: Option<T>, chan: &Channel<T>) -> Self {
        Request {
            handle: handle::current(),
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
        let mut backoff = Backoff::new();
        loop {
            if self.1.load(SeqCst) {
                break;
            }
            backoff.step();
        }
    }
}
