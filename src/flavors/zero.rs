//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};

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
    wait_queues: [Monitor; 2],
    is_closed: AtomicBool,
}

impl Channel {
    #[inline]
    pub fn start_recv(&self, token: &mut Token) -> bool {
        for _ in 0..2 {
            if let Some(case) = self.wait_queues[0].remove_one() {
                unsafe {
                    *token = Token::Case(mem::transmute::<Case, [usize; 2]>(case));
                }
                return true;
            }

            if !self.is_closed() {
                return false;
            }
        }

        *token = Token::Closed;
        true
    }

    pub unsafe fn read<T>(&self, token: &mut Token) -> Option<T> {
        match *token {
            Token::Closed => None,
            Token::Fulfill => Some(self.fulfill_recv()),
            Token::Case(case) => {
                let case: Case = mem::transmute::<[usize; 2], Case>(case);
                Some(finish_exchange(case, None).unwrap())
            }
        }
        // TODO
    }

    pub unsafe fn finish_recv(&self, token: &mut Token) {
        // TODO
    }

    #[inline]
    pub fn start_send(&self, token: &mut Token) -> bool {
        // If there's someone on the other side, exchange messages with it.
        if let Some(case) = self.wait_queues[1].remove_one() {
            unsafe {
                *token = Token::Case(mem::transmute::<Case, [usize; 2]>(case));
            }
            true
        } else {
            false
        }
    }

    pub unsafe fn write<T>(&self, token: &mut Token, msg: T) {
        match *token {
            Token::Closed => unreachable!(),
            Token::Fulfill => self.fulfill_send(msg),
            Token::Case(ref case) => {
                let case: Case = mem::transmute::<[usize; 2], Case>(*case);
                finish_exchange(case, Some(msg));
            }
        }
        // TODO
    }

    pub unsafe fn finish_send(&self, token: &mut Token) {
        // TODO
    }

    /// Returns a new zero-capacity channel.
    #[inline]
    pub fn new() -> Self {
        Channel {
            wait_queues: [Monitor::new(), Monitor::new()],
            is_closed: AtomicBool::new(false),
        }
    }

    /// Returns a reference to the monitor for this channel's senders.
    #[inline]
    pub fn senders(&self) -> &Monitor {
        &self.wait_queues[0]
    }

    /// Returns a reference to the monitor for this channel's receivers.
    #[inline]
    pub fn receivers(&self) -> &Monitor {
        &self.wait_queues[1]
    }

    /// Fulfills the promised send operation.
    pub fn fulfill_send<T>(&self, msg: T) {
        fulfill(Some(msg));
    }

    /// Fulfills the promised receive operation.
    pub fn fulfill_recv<T>(&self) -> T {
        fulfill(None).unwrap()
    }

    /// Closes the exchanger and wakes up all currently blocked operations on it.
    #[inline]
    pub fn close(&self) -> bool {
        if !self.is_closed.swap(true, Ordering::SeqCst) {
            self.wait_queues[0].abort_all();
            self.wait_queues[1].abort_all();
            true
        } else {
            false
        }
    }

    /// Returns `true` if the exchanger is closed.
    #[inline]
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::SeqCst)
    }
}

unsafe fn finish_exchange<T>(case: Case, msg: T) -> T {
    // This is a promise.
    // We must request the message and then wait until the promise is fulfilled.

    // Reset the current thread's selection case.
    handle::current_reset();

    // Create a request on the stack and register it in the owner of this case.
    let req = Request::new(msg);
    case.handle.inner.request_ptr.store(&req as *const _ as usize, Ordering::Release);

    // Wake up the owner of this case.
    case.handle.inner.thread.unpark();

    // Wait until our selection case is woken.
    handle::current_wait_until(None);

    // Extract the received message from the request.
    req.into_msg()
}

/// Fulfills the previously made promise.
fn fulfill<T>(msg: T) -> T {
    // Wait until the requesting thread gives us a pointer to its `Request`.
    let req = HANDLE.with(|handle| {
        let mut backoff = Backoff::new();
        loop {
            let ptr = handle.inner.request_ptr.swap(0, Ordering::Acquire) as *const Request<T>;
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
