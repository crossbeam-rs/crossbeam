use std::collections::VecDeque;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Release};
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use select::CaseId;
use select::handle::{self, Handle};
use util::Backoff;

/// Waiting strategy in an exchange operation.
enum Wait {
    /// Yield once and then time out.
    YieldOnce,

    /// Wait until an optional deadline.
    Until(Option<Instant>),
}

/// Enumeration of possible exchange errors.
pub enum ExchangeError<T> {
    /// The exchange operation timed out.
    Timeout(T),

    /// The exchanger was disconnected.
    Disconnected(T),
}

/// Inner representation of an exchanger.
///
/// This data structure is wrapped in a mutex.
struct Inner<T> {
    /// There are two wait queues, one per side.
    wait_queues: [WaitQueue<T>; 2],

    /// `true` if the exchanger is closed.
    closed: bool,
}

/// A two-sided exchanger.
///
/// This is a concurrent data structure with two sides: left and right. A thread can offer a value
/// on one side, and if there is another thread waiting on the opposite side at the same time, they
/// exchange values.
///
/// Instead of *offering* a concrete value for excahnge, a thread can also *promise* a value.
/// If another thread pairs up with the promise on the opposite end, then it will wait until the
/// promise is fulfilled. The thread that promised the value must in the end either revoke the
/// promise or fulfill it.
pub struct Exchanger<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Exchanger<T> {
    /// Returns a new exchanger.
    pub fn new() -> Self {
        Exchanger {
            inner: Mutex::new(Inner {
                wait_queues: [WaitQueue::new(), WaitQueue::new()],
                closed: false,
            }),
        }
    }

    /// Returns the left side of the exchanger.
    pub fn left(&self) -> Side<T> {
        Side {
            index: 0,
            exchanger: self,
        }
    }

    /// Returns the right side of the exchanger.
    pub fn right(&self) -> Side<T> {
        Side {
            index: 1,
            exchanger: self,
        }
    }

    /// Closes the exchanger.
    pub fn close(&self) -> bool {
        let mut inner = self.inner.lock();

        if inner.closed {
            false
        } else {
            inner.closed = true;
            inner.wait_queues[0].abort_all();
            inner.wait_queues[1].abort_all();
            true
        }
    }

    /// Returns `true` if the exchanger is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }
}

/// One side of an exchanger.
pub struct Side<'a, T: 'a> {
    /// The index is 0 or 1.
    index: usize,

    /// A reference to the parent exchanger.
    exchanger: &'a Exchanger<T>,
}

impl<'a, T> Side<'a, T> {
    /// Promises a value for exchange.
    pub fn promise(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].promise(case_id);
    }

    /// Revokes the previously made promise.
    ///
    /// TODO: what if somebody paired up with it?
    pub fn revoke(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].revoke(case_id);
    }

    /// Fulfills the previously made promise.
    pub fn fulfill(&self, value: T) -> T {
        finish_exchange(value, self.exchanger)
    }

    /// Exchanges `value` if there is an offer or promise on the opposite side.
    pub fn try_exchange(
        &self,
        value: T,
        case_id: CaseId,
    ) -> Result<T, ExchangeError<T>> {
        self.exchange(value, Wait::YieldOnce, case_id)
    }

    /// Exchanges `value`, waiting until the specified `deadline`.
    pub fn exchange_until(
        &self,
        value: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, ExchangeError<T>> {
        self.exchange(value, Wait::Until(deadline), case_id)
    }

    /// Returns `true` if there is an offer or promise on the opposite side.
    pub fn can_notify(&self) -> bool {
        self.exchanger.inner.lock().wait_queues[self.index].can_notify()
    }

    /// Exchanges `value` with the specified `wait` strategy.
    fn exchange(
        &self,
        mut value: T,
        wait: Wait,
        case_id: CaseId,
    ) -> Result<T, ExchangeError<T>> {
        loop {
            // Allocate a packet on the stack.
            let packet;
            {
                let mut inner = self.exchanger.inner.lock();
                if inner.closed {
                    return Err(ExchangeError::Disconnected(value));
                }

                // If there's someone on the other side, exchange values with it.
                if let Some(case) = inner.wait_queues[self.index ^ 1].pop() {
                    drop(inner);
                    return Ok(case.exchange(value, self.exchanger));
                }

                // Promise a packet with the value.
                handle::current_reset();
                packet = Packet::new(value);
                inner.wait_queues[self.index].offer(&packet, case_id);
            }

            let timed_out = match wait {
                Wait::YieldOnce => {
                    thread::yield_now();
                    handle::current_try_select(CaseId::abort())
                },
                Wait::Until(deadline) => {
                    !handle::current_wait_until(deadline)
                },
            };

            // If someone requested the promised value...
            if handle::current_selected() != CaseId::abort() {
                // Wait until the value is taken and return.
                packet.wait();
                return Ok(packet.into_inner());
            }

            // Revoke the promise.
            let mut inner = self.exchanger.inner.lock();
            inner.wait_queues[self.index].revoke(case_id);
            value = packet.into_inner();

            // If we timed out, return.
            if timed_out {
                return Err(ExchangeError::Timeout(value));
            }

            // Otherwise, another thread must have woken us up. Let's try again.
        }
    }
}

enum Case<T> {
    Offer {
        handle: Handle,
        packet: *const Packet<T>,
        case_id: CaseId,
    },
    Promise { local: Arc<Local>, case_id: CaseId },
}

impl<T> Case<T> {
    fn exchange(&self, value: T, exchanger: &Exchanger<T>) -> T {
        match *self {
            Case::Offer {
                ref handle, packet, ..
            } => {
                let v = unsafe { (*packet).exchange(value) };
                handle.unpark();
                v
            }
            Case::Promise { ref local, .. } => {
                handle::current_reset();
                let req = Request::new(value, exchanger);
                local.request_ptr.store(&req as *const _ as usize, Release);
                local.handle.unpark();
                handle::current_wait_until(None);
                req.packet.into_inner()
            }
        }
    }

    fn handle(&self) -> &Handle {
        match *self {
            Case::Offer { ref handle, .. } => handle,
            Case::Promise { ref local, .. } => &local.handle,
        }
    }

    fn case_id(&self) -> CaseId {
        match *self {
            Case::Offer { case_id, .. } => case_id,
            Case::Promise { case_id, .. } => case_id,
        }
    }
}

fn finish_exchange<T>(value: T, exchanger: &Exchanger<T>) -> T {
    let req = loop {
        let ptr = LOCAL.with(|l| l.request_ptr.swap(0, Acquire) as *const Request<T>);
        if !ptr.is_null() {
            break ptr;
        }
        thread::yield_now();
    };

    unsafe {
        assert!((*req).exchanger == exchanger);

        let handle = (*req).handle.clone();
        let v = (*req).packet.exchange(value);
        (*req).handle.try_select(CaseId::abort());
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
                if self.cases[i].handle().try_select(self.cases[i].case_id()) {
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

    fn promise(&mut self, case_id: CaseId) {
        self.cases.push_back(Case::Promise {
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
            case.handle().try_select(CaseId::abort());
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
    exchanger: *const Exchanger<T>,
}

impl<T> Request<T> {
    fn new(value: T, exchanger: &Exchanger<T>) -> Self {
        Request {
            handle: handle::current(),
            packet: Packet::new(value),
            exchanger,
        }
    }
}

struct Packet<T> {
    value: Mutex<Option<T>>,
    ready: AtomicBool,
}

impl<T> Packet<T> {
    fn new(value: T) -> Self {
        Packet {
            value: Mutex::new(Some(value)),
            ready: AtomicBool::new(false),
        }
    }

    fn exchange(&self, value: T) -> T {
        let r = mem::replace(&mut *self.value.try_lock().unwrap(), Some(value));
        self.ready.store(true, Release);
        r.unwrap()
    }

    fn into_inner(self) -> T {
        self.value.try_lock().unwrap().take().unwrap()
    }

    /// Spin-waits until the packet becomes ready.
    fn wait(&self) {
        let mut backoff = Backoff::new();
        while !self.ready.load(Acquire) {
            backoff.step();
        }
    }
}
