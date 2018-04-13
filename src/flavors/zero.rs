//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

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
use utils::Backoff;

// #[derive(Copy, Clone)]
// pub struct Token {
//     boxed: usize,
// }
pub type Token = usize;

/// A zero-capacity channel.
pub struct Channel<T> {
    /// The internal two-sided exchanger.
    exchanger: Exchanger<Option<T>>,
}

impl<T> Channel<T> {
    pub fn sel_try_recv(&self) -> Option<usize> {
        self.exchanger.right().sel_try_exchange()
    }

    pub unsafe fn finish_recv(&self, token: usize) -> Option<T> {
        if token == 0 {
            None
        } else if token == 1 {
            Some(self.fulfill_recv())
        } else {
            Some(self.exchanger.right().finish_exchange(token, None).unwrap())
        }
    }

    pub fn sel_try_send(&self) -> Option<usize> {
        self.exchanger.left().sel_try_exchange()
    }

    pub unsafe fn finish_send(&self, token: usize, msg: T) {
        if token == 1 {
            self.fulfill_send(msg);
        } else {
            self.exchanger.right().finish_exchange(token, Some(msg));
        }
    }

    /// Returns a new zero-capacity channel.
    pub fn new() -> Self {
        Channel {
            exchanger: Exchanger::new(),
        }
    }

    /// Promises a send operation.
    pub fn promise_send(&self, case_id: CaseId) {
        self.exchanger.left().promise(case_id);
    }

    /// Revokes the promised send operation.
    pub fn revoke_send(&self, case_id: CaseId) {
        self.exchanger.left().revoke(case_id);
    }

    /// Fulfills the promised send operation.
    pub fn fulfill_send(&self, msg: T) {
        self.exchanger.left().fulfill(Some(msg));
    }

    /// Promises a receive operation.
    pub fn promise_recv(&self, case_id: CaseId) {
        self.exchanger.right().promise(case_id);
    }

    /// Revokes the promised receive operation.
    pub fn revoke_recv(&self, case_id: CaseId) {
        self.exchanger.right().revoke(case_id);
    }

    /// Fulfills the promised receive operation.
    pub fn fulfill_recv(&self) -> T {
        self.exchanger.right().fulfill(None).unwrap()
    }

    /// Attempts to send `msg` into the channel.
    pub fn try_send(&self, msg: T, case_id: CaseId) -> Option<T> {
        match self.exchanger.left().try_exchange(Some(msg), case_id) {
            Ok(_) => None,
            Err(ExchangeError::Timeout(msg)) => Some(msg.unwrap()),
            Err(ExchangeError::Closed(msg)) => panic!(), // TODO: delete this case
        }
    }

    /// Attempts to send `msg` into the channel.
    pub fn send(&self, msg: T, case_id: CaseId) {
        match self.exchanger
            .left()
            .exchange_until(Some(msg), None, case_id)
        {
            Ok(_) => (),
            Err(ExchangeError::Closed(msg)) => panic!(), // TODO: delete this case
            Err(ExchangeError::Timeout(msg)) => panic!(), // TODO: cannot happen?
        }
    }

    /// Attempts to receive a message from channel.
    pub fn try_recv(&self, case_id: CaseId) -> Option<T> {
        match self.exchanger.right().try_exchange(None, case_id) {
            Ok(msg) => Some(msg.unwrap()),
            Err(ExchangeError::Closed(_)) => None,
            Err(ExchangeError::Timeout(_)) => None,
        }
    }

    /// Attempts to receive a message from the channel.
    pub fn recv(
        &self,
        case_id: CaseId,
    ) -> Option<T> {
        match self.exchanger
            .right()
            .exchange_until(None, None, case_id)
        {
            Ok(msg) => Some(msg.unwrap()),
            Err(ExchangeError::Closed(_)) => None,
            Err(ExchangeError::Timeout(_)) => panic!(), // TODO: cannot happeN?
        }
    }

    /// Returns `true` if there is a waiting sender.
    pub fn can_recv(&self) -> bool {
        self.exchanger.left().can_notify()
    }

    /// Returns `true` if there is a waiting receiver.
    pub fn can_send(&self) -> bool {
        self.exchanger.right().can_notify()
    }

    /// Closes the channel and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        self.exchanger.close()
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.exchanger.is_closed()
    }
}

/// Waiting strategy in an exchange operation.
enum Wait {
    /// Yield once and then time out.
    YieldOnce,

    /// Wait until an optional deadline.
    Until(Option<Instant>),
}

/// Enumeration of possible exchange errors.
enum ExchangeError<T> {
    /// The exchange operation timed out.
    Timeout(T),

    /// The exchanger is closed.
    Closed(T),
}

/// Inner representation of an exchanger.
///
/// This data structure is wrapped in a mutex.
struct Inner<T> {
    /// There are two wait queues, one per side.
    wait_queues: [WaitQueue<T>; 2],

    /// `true` if the exchanger is closed.
    is_closed: bool,
}

/// A two-sided exchanger.
///
/// This is a concurrent data structure with two sides: left and right. A thread can offer a
/// message on one side, and if there is another thread waiting on the opposite side at the same
/// time, they exchange messages.
///
/// Instead of *offering* a concrete message for excahnge, a thread can also *promise* a message.
/// If another thread pairs up with the promise on the opposite end, then it will wait until the
/// promise is fulfilled. The thread that promised the message must in the end fulfill it. A
/// promise can also be revoked if nobody is waiting for it.
struct Exchanger<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Exchanger<T> {
    /// Returns a new exchanger.
    #[inline]
    fn new() -> Self {
        Exchanger {
            inner: Mutex::new(Inner {
                wait_queues: [WaitQueue::new(), WaitQueue::new()],
                is_closed: false,
            }),
        }
    }

    /// Returns the left side of the exchanger.
    fn left(&self) -> Side<T> {
        Side {
            index: 0,
            exchanger: self,
        }
    }

    /// Returns the right side of the exchanger.
    fn right(&self) -> Side<T> {
        Side {
            index: 1,
            exchanger: self,
        }
    }

    /// Closes the exchanger and wakes up all currently blocked operations on it.
    fn close(&self) -> bool {
        let mut inner = self.inner.lock();

        if inner.is_closed {
            false
        } else {
            inner.is_closed = true;
            inner.wait_queues[0].abort_all();
            inner.wait_queues[1].abort_all();
            true
        }
    }

    /// Returns `true` if the exchanger is closed.
    fn is_closed(&self) -> bool {
        self.inner.lock().is_closed
    }
}

/// One side of an exchanger.
struct Side<'a, T: 'a> {
    /// The index is 0 or 1.
    index: usize,

    /// A reference to the parent exchanger.
    exchanger: &'a Exchanger<T>,
}

impl<'a, T> Side<'a, T> {
    fn sel_try_exchange(&self) -> Option<usize> {
        let mut inner = self.exchanger.inner.lock();
        if inner.is_closed {
            return Some(0);
        }

        // If there's someone on the other side, exchange messages with it.
        if let Some(case) = inner.wait_queues[self.index ^ 1].pop() {
            drop(inner);

            let entry: Box<Entry<T>> = Box::new(case); // TODO: optimize
            let entry = Box::into_raw(entry) as usize;
            return Some(entry);
        }

        None
    }

    unsafe fn finish_exchange(&self, token: usize, msg: T) -> T {
        let entry = *Box::from_raw(token as *mut Entry<T>);
        entry.exchange(msg)
    }

    /// Promises a message for exchange.
    fn promise(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].promise(case_id);
    }

    /// Revokes the previously made promise.
    ///
    /// This method should be called right after the case is aborted.
    fn revoke(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].remove(case_id);
    }

    /// Fulfills the previously made promise.
    fn fulfill(&self, msg: T) -> T {
        // Wait until the requesting thread gives us a pointer to its `Request`.
        let req = LOCAL.with(|local| {
            let mut backoff = Backoff::new();
            loop {
                let ptr = local.request_ptr.swap(0, Acquire) as *const Request<T>;
                if !ptr.is_null() {
                    break ptr;
                }
                backoff.step();
            }
        });

        unsafe {
            // First, make a clone of the requesting thread's `Handle`.
            let handle = (*req).handle.clone();

            // Exchange the messages and then notify the requesting thread that it can pick up our
            // message.
            let m = (*req).packet.exchange(msg);
            (*req).handle.try_select(CaseId::abort());

            // Wake up the requesting thread.
            handle.unpark();

            // Return the exchanged message.
            m
        }
    }

    /// Exchanges `msg` if there is an offer or promise on the opposite side.
    fn try_exchange(&self, msg: T, case_id: CaseId) -> Result<T, ExchangeError<T>> {
        self.exchange(msg, Wait::YieldOnce, case_id)
    }

    /// Exchanges `msg`, waiting until the specified `deadline`.
    fn exchange_until(
        &self,
        msg: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, ExchangeError<T>> {
        self.exchange(msg, Wait::Until(deadline), case_id)
    }

    /// Returns `true` if there is an offer or promise on the opposite side.
    fn can_notify(&self) -> bool {
        self.exchanger.inner.lock().wait_queues[self.index].can_notify()
    }

    /// Exchanges `msg` with the specified `wait` strategy.
    fn exchange(&self, mut msg: T, wait: Wait, case_id: CaseId) -> Result<T, ExchangeError<T>> {
        loop {
            // Allocate a packet on the stack.
            let packet;
            {
                let mut inner = self.exchanger.inner.lock();
                if inner.is_closed {
                    return Err(ExchangeError::Closed(msg));
                }

                // If there's someone on the other side, exchange messages with it.
                if let Some(case) = inner.wait_queues[self.index ^ 1].pop() {
                    drop(inner);
                    return Ok(case.exchange(msg));
                }

                // Promise a packet with the message.
                handle::current_reset();
                packet = Packet::new(msg);
                inner.wait_queues[self.index].offer(&packet, case_id);
            }

            // Wait until the timeout and then try aborting the case.
            let timed_out = match wait {
                Wait::YieldOnce => {
                    thread::yield_now();
                    handle::current_try_select(CaseId::abort())
                }
                Wait::Until(deadline) => !handle::current_wait_until(deadline),
            };

            // If someone requested the promised message...
            if handle::current_selected() != CaseId::abort() {
                // Wait until the message is taken and return.
                packet.wait();
                return Ok(packet.into_inner());
            }

            // Revoke the promise.
            let mut inner = self.exchanger.inner.lock();
            inner.wait_queues[self.index].remove(case_id);
            msg = packet.into_inner();

            // If we timed out, return.
            if timed_out {
                return Err(ExchangeError::Timeout(msg));
            }

            // Otherwise, another thread must have woken us up. Let's try again.
        }
    }
}

/// An entry in a wait queue.
enum Entry<T> {
    /// Offers a concrete message.
    Offer {
        handle: Handle,
        packet: *const Packet<T>,
        case_id: CaseId,
    },
    /// Promises a message.
    Promise { local: Arc<Local>, case_id: CaseId },
}

impl<T> Entry<T> {
    /// Exchange `msg` with this entry and wake it up.
    fn exchange(&self, msg: T) -> T {
        match *self {
            // This is an offer.
            // We can exchange messages immediately.
            Entry::Offer {
                ref handle, packet, ..
            } => {
                let m = unsafe { (*packet).exchange(msg) };
                handle.unpark();
                m
            }

            // This is a promise.
            // We must request the message and then wait until the promise is fulfilled.
            Entry::Promise { ref local, .. } => {
                // Reset the current thread's selection case.
                handle::current_reset();

                // Create a request on the stack and register it in the owner of this entry.
                let req = Request::new(msg);
                local.request_ptr.store(&req as *const _ as usize, Release);

                // Wake up the owner of this entry.
                local.handle.unpark();

                // Wait until our selection case is woken.
                handle::current_wait_until(None);

                // Extract the received message from the request.
                req.packet.into_inner()
            }
        }
    }

    /// Returns the handle associated with the owner of this entry.
    fn handle(&self) -> &Handle {
        match *self {
            Entry::Offer { ref handle, .. } => handle,
            Entry::Promise { ref local, .. } => &local.handle,
        }
    }

    /// Returns the case ID associated with this entry.
    fn case_id(&self) -> CaseId {
        match *self {
            Entry::Offer { case_id, .. } | Entry::Promise { case_id, .. } => case_id,
        }
    }
}

/// A wait queue in which blocked selection cases are registered.
struct WaitQueue<T> {
    cases: VecDeque<Entry<T>>,
}

impl<T> WaitQueue<T> {
    /// Creates a new `WaitQueue`.
    fn new() -> Self {
        WaitQueue {
            cases: VecDeque::new(),
        }
    }

    /// Attempts to fire one case owned by another thread and returns it on success.
    fn pop(&mut self) -> Option<Entry<T>> {
        let thread_id = current_thread_id();

        for i in 0..self.cases.len() {
            if self.cases[i].handle().thread_id() != thread_id {
                if self.cases[i].handle().try_select(self.cases[i].case_id()) {
                    let case = self.cases.remove(i).unwrap();
                    self.maybe_shrink();
                    return Some(case);
                }
            }
        }

        None
    }

    /// Inserts an *offer* case owned by the current thread with `case_id`.
    fn offer(&mut self, packet: *const Packet<T>, case_id: CaseId) {
        self.cases.push_back(Entry::Offer {
            handle: handle::current(),
            packet,
            case_id,
        });
    }

    /// Inserts a *promise* case owned by the current thread with `case_id`.
    fn promise(&mut self, case_id: CaseId) {
        self.cases.push_back(Entry::Promise {
            local: LOCAL.with(|l| l.clone()),
            case_id,
        });
    }

    /// Removes a case owned by the current thread with `case_id`.
    fn remove(&mut self, case_id: CaseId) {
        let thread_id = current_thread_id();

        if let Some((i, _)) = self.cases
            .iter()
            .enumerate()
            .find(|&(_, case)| case.case_id() == case_id && case.handle().thread_id() == thread_id)
        {
            self.cases.remove(i);
            self.maybe_shrink();
        }
    }

    /// Returns `true` if there exists a case which isn't owned by the current thread.
    fn can_notify(&self) -> bool {
        let thread_id = current_thread_id();

        for i in 0..self.cases.len() {
            if self.cases[i].handle().thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    /// Aborts all cases and unparks threads which own them.
    fn abort_all(&mut self) {
        for case in self.cases.drain(..) {
            case.handle().try_select(CaseId::abort());
            case.handle().unpark();
        }
        self.maybe_shrink();
    }

    /// Shrinks the internal buffer if its capacity is underused.
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

/// Returns the current thread ID.
///
/// This is much faster than calling `std::thread::current().id()`.
fn current_thread_id() -> thread::ThreadId {
    THREAD_ID.with(|id| *id)
}

thread_local! {
    static LOCAL: Arc<Local> = Arc::new(Local {
        handle: handle::current(),
        request_ptr: AtomicUsize::new(0),
    });
}

/// Thread-local structure that contains a slot for the `Request` pointer.
struct Local {
    /// The handle associated with this thread.
    handle: Handle,

    /// A slot into which another thread may store a pointer to its `Request`.
    request_ptr: AtomicUsize,
}

/// A request for promised message.
struct Request<T> {
    /// The handle associated with the requestor.
    handle: Handle,

    /// The message for exchange.
    packet: Packet<T>,
}

impl<T> Request<T> {
    /// Creates a new request owned by the current thread for exchanging `msg`.
    fn new(msg: T) -> Self {
        Request {
            handle: handle::current(),
            packet: Packet::new(msg),
        }
    }
}

/// A one-time only packet for exchanging messages.
///
/// A message can be exchanged for the one inside the packet only once.
struct Packet<T> {
    /// The mutex-protected message.
    msg: Mutex<Option<T>>,

    /// This will become `true` when the message is exchanged.
    ready: AtomicBool,
}

impl<T> Packet<T> {
    /// Creates a new packet containing `msg`.
    fn new(msg: T) -> Self {
        Packet {
            msg: Mutex::new(Some(msg)),
            ready: AtomicBool::new(false),
        }
    }

    /// Exchanges `msg` for the one inside the packet.
    fn exchange(&self, msg: T) -> T {
        let r = mem::replace(&mut *self.msg.try_lock().unwrap(), Some(msg));
        self.ready.store(true, Release);
        r.unwrap()
    }

    /// Extracts the message inside the packet.
    fn into_inner(self) -> T {
        self.msg.try_lock().unwrap().take().unwrap()
    }

    /// Spin-waits until the message inside the packet is exchanged.
    fn wait(&self) {
        let mut backoff = Backoff::new();
        while !self.ready.load(Acquire) {
            backoff.step();
        }
    }
}
