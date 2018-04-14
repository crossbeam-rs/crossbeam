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
pub struct Channel {
    /// The internal two-sided exchanger.
    exchanger: Exchanger,
}

impl Channel {
    #[inline]
    pub fn sel_try_recv(&self, token: &mut Token) -> bool {
        self.exchanger.right().sel_try_exchange(token)
    }

    pub unsafe fn finish_recv<T>(&self, token: usize) -> Option<T> {
        if token == 0 {
            None
        } else if token == 1 {
            Some(self.fulfill_recv())
        } else {
            Some(self.exchanger.right().finish_exchange(token, None).unwrap())
        }
    }

    #[inline]
    pub fn sel_try_send(&self, token: &mut Token) -> bool {
        self.exchanger.left().sel_try_exchange(token)
    }

    pub unsafe fn finish_send<T>(&self, token: usize, msg: T) {
        if token == 1 {
            self.fulfill_send(msg);
        } else {
            self.exchanger.right().finish_exchange(token, Some(msg));
        }
    }

    /// Returns a new zero-capacity channel.
    #[inline]
    pub fn new() -> Self {
        Channel {
            exchanger: Exchanger::new(),
        }
    }

    /// Promises a send operation.
    #[inline]
    pub fn promise_send(&self, case_id: CaseId) {
        self.exchanger.left().promise(case_id);
    }

    /// Revokes the promised send operation.
    #[inline]
    pub fn revoke_send(&self, case_id: CaseId) {
        self.exchanger.left().revoke(case_id);
    }

    /// Fulfills the promised send operation.
    pub fn fulfill_send<T>(&self, msg: T) {
        self.exchanger.left().fulfill(Some(msg));
    }

    /// Promises a receive operation.
    #[inline]
    pub fn promise_recv(&self, case_id: CaseId) {
        self.exchanger.right().promise(case_id);
    }

    /// Revokes the promised receive operation.
    #[inline]
    pub fn revoke_recv(&self, case_id: CaseId) {
        self.exchanger.right().revoke(case_id);
    }

    /// Fulfills the promised receive operation.
    pub fn fulfill_recv<T>(&self) -> T {
        self.exchanger.right().fulfill(None).unwrap()
    }

    /// Returns `true` if there is a waiting sender.
    #[inline]
    pub fn can_recv(&self) -> bool {
        self.exchanger.left().can_notify()
    }

    /// Returns `true` if there is a waiting receiver.
    #[inline]
    pub fn can_send(&self) -> bool {
        self.exchanger.right().can_notify()
    }

    /// Closes the channel and wakes up all currently blocked operations on it.
    #[inline]
    pub fn close(&self) -> bool {
        self.exchanger.close()
    }

    /// Returns `true` if the channel is closed.
    #[inline]
    pub fn is_closed(&self) -> bool {
        self.exchanger.is_closed()
    }
}

/// Inner representation of an exchanger.
///
/// This data structure is wrapped in a mutex.
struct Inner {
    /// There are two wait queues, one per side.
    wait_queues: [WaitQueue; 2],

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
struct Exchanger {
    inner: Mutex<Inner>,
}

impl Exchanger {
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
    #[inline]
    fn left(&self) -> Side {
        Side {
            index: 0,
            exchanger: self,
        }
    }

    /// Returns the right side of the exchanger.
    #[inline]
    fn right(&self) -> Side {
        Side {
            index: 1,
            exchanger: self,
        }
    }

    /// Closes the exchanger and wakes up all currently blocked operations on it.
    #[inline]
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
    #[inline]
    fn is_closed(&self) -> bool {
        self.inner.lock().is_closed
    }
}

/// One side of an exchanger.
struct Side<'a> {
    /// The index is 0 or 1.
    index: usize,

    /// A reference to the parent exchanger.
    exchanger: &'a Exchanger,
}

impl<'a> Side<'a> {
    #[inline]
    fn sel_try_exchange(&self, token: &mut Token) -> bool {
        let mut inner = self.exchanger.inner.lock();
        if inner.is_closed {
            *token = 0;
            return true;
        }

        // If there's someone on the other side, exchange messages with it.
        if let Some(case) = inner.wait_queues[self.index ^ 1].pop() {
            drop(inner);

            let entry: Box<Entry> = Box::new(case); // TODO: optimize
            let entry = Box::into_raw(entry) as usize;
            *token = entry;
            return true;
        }

        false
    }

    unsafe fn finish_exchange<T>(&self, token: usize, msg: T) -> T {
        let entry = *Box::from_raw(token as *mut Entry);
        entry.exchange(msg)
    }

    /// Promises a message for exchange.
    #[inline]
    fn promise(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].promise(case_id);
    }

    /// Revokes the previously made promise.
    ///
    /// This method should be called right after the case is aborted.
    #[inline]
    fn revoke(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].remove(case_id);
    }

    /// Fulfills the previously made promise.
    fn fulfill<T>(&self, msg: T) -> T {
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

    /// Returns `true` if there is an offer or promise on the opposite side.
    #[inline]
    fn can_notify(&self) -> bool {
        self.exchanger.inner.lock().wait_queues[self.index].can_notify()
    }
}

/// An entry in a wait queue.
enum Entry {
    /// Promises a message.
    Promise { local: Arc<Local>, case_id: CaseId },
}

impl Entry {
    /// Exchange `msg` with this entry and wake it up.
    fn exchange<T>(&self, msg: T) -> T {
        match *self {
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
    #[inline]
    fn handle(&self) -> &Handle {
        match *self {
            Entry::Promise { ref local, .. } => &local.handle,
        }
    }

    /// Returns the case ID associated with this entry.
    #[inline]
    fn case_id(&self) -> CaseId {
        match *self {
            Entry::Promise { case_id, .. } => case_id,
        }
    }
}

/// A wait queue in which blocked selection cases are registered.
struct WaitQueue {
    cases: VecDeque<Entry>,
}

impl WaitQueue {
    /// Creates a new `WaitQueue`.
    #[inline]
    fn new() -> Self {
        WaitQueue {
            cases: VecDeque::new(),
        }
    }

    /// Attempts to fire one case owned by another thread and returns it on success.
    #[inline]
    fn pop(&mut self) -> Option<Entry> {
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

    /// Inserts a *promise* case owned by the current thread with `case_id`.
    #[inline]
    fn promise(&mut self, case_id: CaseId) {
        self.cases.push_back(Entry::Promise {
            local: LOCAL.with(|l| l.clone()),
            case_id,
        });
    }

    /// Removes a case owned by the current thread with `case_id`.
    #[inline]
    fn remove(&mut self, case_id: CaseId) {
        let thread_id = current_thread_id();

        if let Some((i, _)) = self.cases
            .iter()
            .enumerate()
            .find(|&(_, case)| case.case_id() == case_id)
        {
            self.cases.remove(i);
            self.maybe_shrink();
        }
    }

    /// Returns `true` if there exists a case which isn't owned by the current thread.
    #[inline]
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
    #[inline]
    fn abort_all(&mut self) {
        for case in self.cases.drain(..) {
            case.handle().try_select(CaseId::abort());
            case.handle().unpark();
        }
        self.maybe_shrink();
    }

    /// Shrinks the internal buffer if its capacity is underused.
    #[inline]
    fn maybe_shrink(&mut self) {
        if self.cases.capacity() > 32 && self.cases.capacity() / 2 > self.cases.len() {
            self.cases.shrink_to_fit();
        }
    }
}

impl Drop for WaitQueue {
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
