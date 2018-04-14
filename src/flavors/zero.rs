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

use monitor::{Monitor, Case};
use select::CaseId;
use select::handle::{self, HANDLE, Handle};
use utils::Backoff;

#[derive(Copy, Clone)]
pub enum Token {
    Closed,
    Fulfill,
    Case([usize; 2]),
}

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

    pub unsafe fn finish_recv<T>(&self, token: Token) -> Option<T> {
        match token {
            Token::Closed => None,
            Token::Fulfill => Some(self.fulfill_recv()),
            Token::Case(case) => {
                let case: Case = mem::transmute(case);
                Some(self.exchanger.right().finish_exchange(case, None).unwrap())
            }
        }
    }

    #[inline]
    pub fn sel_try_send(&self, token: &mut Token) -> bool {
        self.exchanger.left().sel_try_exchange(token)
    }

    pub unsafe fn finish_send<T>(&self, token: Token, msg: T) {
        match token {
            Token::Closed => unreachable!(),
            Token::Fulfill => self.fulfill_send(msg),
            Token::Case(case) => {
                let case: Case = mem::transmute(case);
                self.exchanger.right().finish_exchange(case, Some(msg));
            }
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
    wait_queues: [Monitor; 2],

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
                wait_queues: [Monitor::new(), Monitor::new()],
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
            *token = Token::Closed;
            return true;
        }

        // If there's someone on the other side, exchange messages with it.
        if let Some(case) = inner.wait_queues[self.index ^ 1].remove_one() {
            drop(inner);

            unsafe {
                *token = Token::Case(mem::transmute(case));
            }
            return true;
        }

        false
    }

    unsafe fn finish_exchange<T>(&self, case: Case, msg: T) -> T {
        // This is a promise.
        // We must request the message and then wait until the promise is fulfilled.

        // Reset the current thread's selection case.
        handle::current_reset();

        // Create a request on the stack and register it in the owner of this case.
        let req = Request::new(msg);
        case.handle.inner.request_ptr.store(&req as *const _ as usize, Release);

        // Wake up the owner of this case.
        case.handle.inner.thread.unpark();

        // Wait until our selection case is woken.
        handle::current_wait_until(None);

        // Extract the received message from the request.
        req.into_msg()
    }

    /// Promises a message for exchange.
    #[inline]
    fn promise(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].register(case_id);
    }

    /// Revokes the previously made promise.
    ///
    /// This method should be called right after the case is aborted.
    #[inline]
    fn revoke(&self, case_id: CaseId) {
        self.exchanger.inner.lock().wait_queues[self.index].unregister(case_id);
    }

    /// Fulfills the previously made promise.
    fn fulfill<T>(&self, msg: T) -> T {
        // Wait until the requesting thread gives us a pointer to its `Request`.
        let req = HANDLE.with(|handle| {
            let mut backoff = Backoff::new();
            loop {
                let ptr = handle.inner.request_ptr.swap(0, Acquire) as *const Request<T>;
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
            let m = (*req).exchange(msg);
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

/// A request for promised message.
struct Request<T> {
    /// The handle associated with the requestor.
    handle: Handle,

    /// The message for exchange.
    msg: Mutex<Option<T>>,
}

impl<T> Request<T> {
    /// Creates a new request owned by the current thread for exchanging `msg`.
    fn new(msg: T) -> Self {
        Request {
            handle: handle::current(),
            msg: Mutex::new(Some(msg)),
        }
    }

    /// Exchanges `msg` for the one inside the packet.
    fn exchange(&self, msg: T) -> T {
        let r = mem::replace(&mut *self.msg.try_lock().unwrap(), Some(msg));
        r.unwrap()
    }

    /// Extracts the message inside the packet.
    fn into_msg(self) -> T {
        self.msg.try_lock().unwrap().take().unwrap()
    }
}
