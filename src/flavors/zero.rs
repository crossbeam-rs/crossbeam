use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Release, SeqCst};
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use select::CaseId;
use select::handle::{self, Handle};
use util::Backoff;

// TODO: check seqcst orderings
// TODO: move Monitor and Exchanger into wait::*;
// TODO: Make packet thread-local?

enum Effort {
    Try,
    Yield,
    Until(Option<Instant>),
}

struct Inner<T> {
    senders: WaitQueue<T>,
    receivers: WaitQueue<T>,
    closed: bool,
}

pub struct Channel<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        Channel {
            inner: Mutex::new(Inner {
                senders: WaitQueue::new(),
                receivers: WaitQueue::new(),
                closed: false,
            }),
        }
    }

    pub fn promise_send(&self, case_id: CaseId) {
        self.inner.lock().senders.bond(case_id);
    }

    pub fn revoke_send(&self, case_id: CaseId) {
        self.inner.lock().senders.revoke(case_id);
    }

    pub fn fulfill_send(&self, value: T) {
        finish_send(value, self)
    }

    pub fn promise_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.bond(case_id);
    }

    pub fn revoke_recv(&self, case_id: CaseId) {
        self.inner.lock().receivers.revoke(case_id);
    }

    pub fn fulfill_recv(&self) -> T {
        finish_recv(self)
    }

    pub fn try_send(&self, value: T, case_id: CaseId) -> Result<(), TrySendError<T>> {
        match self.send(value, Effort::Try, case_id) {
            Ok(()) => Ok(()),
            Err(SendTimeoutError::Disconnected(v)) => Err(TrySendError::Disconnected(v)),
            Err(SendTimeoutError::Timeout(v)) => Err(TrySendError::Full(v)),
        }
    }

    pub fn spin_try_send(&self, value: T, case_id: CaseId) -> Result<(), TrySendError<T>> {
        match self.send(value, Effort::Yield, case_id) {
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
        self.send(value, Effort::Until(deadline), case_id)
    }

    fn send(
        &self,
        mut value: T,
        effort: Effort,
        case_id: CaseId,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(SendTimeoutError::Disconnected(value));
                }

                // If there's a waiting receiver, pass it the value.
                if let Some(case) = inner.receivers.pop() {
                    drop(inner);
                    case.send(value, self);
                    return Ok(());
                }

                // If we're allowed to try just once, give up.
                if let Effort::Try = effort {
                    return Err(SendTimeoutError::Timeout(value));
                }

                // Promise a packet with the value.
                handle::current_reset();
                packet = Packet::new(Some(value));
                inner.senders.offer(&packet, case_id);
            }

            let timed_out = if let Effort::Until(deadline) = effort {
                !handle::current_wait_until(deadline)
            } else {
                // Yield once and then abort.
                thread::yield_now();
                handle::current_select(CaseId::abort())
            };

            // If someone requested the promised value...
            if handle::current_selected() != CaseId::abort() {
                // Wait until the value is taken and return.
                packet.wait();
                return Ok(());
            }

            // Revoke the promise.
            let mut inner = self.inner.lock();
            inner.senders.revoke(case_id);
            value = packet.take();

            // If we timed out, return.
            if timed_out {
                return Err(SendTimeoutError::Timeout(value));
            }

            // Otherwise, another thread must have woken us up. Let's try again.
        }
    }

    pub fn try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.recv(Effort::Try, case_id) {
            Ok(v) => Ok(v),
            Err(RecvTimeoutError::Disconnected) => Err(TryRecvError::Disconnected),
            Err(RecvTimeoutError::Timeout) => Err(TryRecvError::Empty),
        }
    }

    pub fn spin_try_recv(&self, case_id: CaseId) -> Result<T, TryRecvError> {
        match self.recv(Effort::Yield, case_id) {
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
        self.recv(Effort::Until(deadline), case_id)
    }

    fn recv(
        &self,
        effort: Effort,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        loop {
            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(RecvTimeoutError::Disconnected);
                }

                // If there's a waiting sender, take its value.
                if let Some(case) = inner.senders.pop() {
                    drop(inner);
                    return Ok(case.recv(self));
                }

                // If we're allowed to try just once, give up.
                if let Effort::Try = effort {
                    return Err(RecvTimeoutError::Timeout);
                }

                // Promise a packet requesting a value.
                handle::current_reset();
                packet = Packet::new(None);
                inner.receivers.offer(&packet, case_id);
            }

            let timed_out = if let Effort::Until(deadline) = effort {
                !handle::current_wait_until(deadline)
            } else {
                // Yield once and then abort.
                thread::yield_now();
                handle::current_select(CaseId::abort())
            };

            // If someone decided to pass us a value...
            if handle::current_selected() != CaseId::abort() {
                // Wait until the value is ready, take it, and return.
                packet.wait();
                return Ok(packet.take());
            }

            // Revoke the promise.
            let mut inner = self.inner.lock();
            inner.receivers.revoke(case_id);

            // If we timed out, return.
            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }

            // Otherwise, another thread must have woken us up. Let's try again.
        }
    }

    pub fn can_recv(&self) -> bool {
        self.inner.lock().senders.can_notify()
    }

    pub fn can_send(&self) -> bool {
        self.inner.lock().receivers.can_notify()
    }

    /// Closes the channel.
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

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }
}

enum Case<T> {
    Offer {
        handle: Handle,
        packet: *const Packet<T>,
        case_id: CaseId,
    },
    Bond { local: Arc<Local>, case_id: CaseId },
}

impl<T> Case<T> {
    fn send(&self, value: T, chan: &Channel<T>) {
        match *self {
            Case::Offer {
                ref handle, packet, ..
            } => {
                unsafe { (*packet).put(value) }
                handle.unpark();
            }
            Case::Bond { ref local, .. } => {
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
            Case::Offer {
                ref handle, packet, ..
            } => {
                let v = unsafe { (*packet).take() };
                // The handle has to lock `self.inner` before continuing.
                handle.unpark();
                v
            }
            Case::Bond { ref local, .. } => {
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
            Case::Offer { ref handle, .. } => handle,
            Case::Bond { ref local, .. } => &local.handle,
        }
    }

    fn case_id(&self) -> CaseId {
        match *self {
            Case::Offer { case_id, .. } => case_id,
            Case::Bond { case_id, .. } => case_id,
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

struct WaitQueue<T> {
    cases: VecDeque<Case<T>>,
}

impl<T> WaitQueue<T> {
    fn new() -> Self {
        WaitQueue {
            cases: VecDeque::new(),
        }
    }

    fn pop(&mut self) -> Option<Case<T>> {
        let thread_id = current_thread_id();

        for i in 0..self.cases.len() {
            if self.cases[i].handle().thread_id() != thread_id {
                if self.cases[i].handle().select(self.cases[i].case_id()) {
                    return Some(self.cases.remove(i).unwrap());
                }
            }
        }

        None
    }

    fn offer(&mut self, packet: *const Packet<T>, case_id: CaseId) {
        self.cases.push_back(Case::Offer {
            handle: handle::current(),
            packet,
            case_id,
        });
    }

    fn bond(&mut self, case_id: CaseId) {
        self.cases.push_back(Case::Bond {
            local: LOCAL.with(|l| l.clone()),
            case_id,
        });
    }

    fn revoke(&mut self, case_id: CaseId) {
        let thread_id = current_thread_id();

        if let Some((i, _)) = self.cases.iter().enumerate().find(|&(_, case)| {
            case.case_id() == case_id && case.handle().thread_id() == thread_id
        }) {
            self.cases.remove(i);
            self.maybe_shrink();
        }
    }

    fn can_notify(&self) -> bool {
        let thread_id = current_thread_id();

        for i in 0..self.cases.len() {
            if self.cases[i].handle().thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    fn abort_all(&mut self) {
        for case in self.cases.drain(..) {
            case.handle().select(CaseId::abort());
            case.handle().unpark();
        }
        self.maybe_shrink();
    }

    fn maybe_shrink(&mut self) {
        if self.cases.capacity() > 32 && self.cases.capacity() / 2 > self.cases.len() {
            self.cases.shrink_to_fit();
        }
    }
}

impl<T> Drop for WaitQueue<T> {
    fn drop(&mut self) {
        debug_assert!(self.cases.is_empty());
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

/// A packet for transferring data.
struct Packet<T> {
    data: Mutex<Option<T>>,
    ready: AtomicBool,
}

impl<T> Packet<T> {
    fn new(data: Option<T>) -> Self {
        Packet {
            data: Mutex::new(data),
            ready: AtomicBool::new(false),
        }
    }

    /// Puts data into the packet and marks it as ready.
    fn put(&self, data: T) {
        {
            let mut opt = self.data.try_lock().unwrap();
            assert!(opt.is_none());
            *opt = Some(data);
        }
        self.ready.store(true, Release);
    }

    /// Takes data from the packet and marks it as ready.
    fn take(&self) -> T {
        let t = self.data.try_lock().unwrap().take().unwrap();
        self.ready.store(true, Release);
        t
    }

    /// Waits until the packet becomes ready.
    fn wait(&self) {
        let mut backoff = Backoff::new();
        while !self.ready.load(Acquire) {
            backoff.step();
        }
    }
}
