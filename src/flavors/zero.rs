//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use channel::Sel;
use select::CaseId;
use select::handle::{self, HANDLE, Handle};
use utils::Backoff;
use waker::{Case, Waker};

pub struct Receiver<'a>(&'a Channel);
pub struct Sender<'a>(&'a Channel);
pub struct PreparedSender<'a>(&'a Channel);

impl<'a> Sel for Receiver<'a> {
    type Token = Token;

    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            self.0.start_recv(token)
        }
    }

    fn promise(&self, case_id: CaseId) {
        self.0.receivers().register(case_id, true)
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        !self.0.senders().can_notify() && !self.0.is_closed()
    }

    fn revoke(&self, case_id: CaseId) {
        self.0.receivers().unregister(case_id);
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            self.0.fulfill_recv(token)
        }
    }

    fn finish(&self, token: &mut Token) {
        unsafe {
            self.0.finish_recv(token);
        }
    }

    fn fail(&self, token: &mut Token) {
        unreachable!();
    }
}

impl<'a> Sel for Sender<'a> {
    type Token = Token;

    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            self.0.start_send(token)
        }
    }

    fn promise(&self, case_id: CaseId) {
        self.0.senders().register(case_id, false)
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        !self.0.receivers().can_notify()
    }

    fn revoke(&self, case_id: CaseId) {
        self.0.senders().unregister(case_id);
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            self.0.fulfill_send(token, false)
        }
    }

    fn finish(&self, token: &mut Token) {
        unsafe {
            self.0.finish_recv(token); // TODO: may fail!
        }
    }

    fn fail(&self, token: &mut Token) {
        unsafe {
            self.0.fail_send(token);
        }
    }
}

impl<'a> Sel for PreparedSender<'a> {
    type Token = Token;

    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            self.0.start_send(token)
        }
    }

    fn promise(&self, case_id: CaseId) {
        self.0.senders().register(case_id, true)
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        !self.0.receivers().can_notify()
    }

    fn revoke(&self, case_id: CaseId) {
        self.0.senders().unregister(case_id);
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            self.0.fulfill_send(token, true)
        }
    }

    fn finish(&self, token: &mut Token) {
        unsafe {
            self.0.finish_recv(token); // TODO: may fail!
        }
    }

    fn fail(&self, token: &mut Token) {
        unreachable!()
    }
}

#[derive(Copy, Clone)]
pub enum Token {
    Closed,
    Fulfill,
    Case([usize; 3]), // TODO: use [u8; mem::size_of::<Case>()]
}

/// A zero-capacity channel.
pub struct Channel {
    wait_queues: [Waker; 2],
    is_closed: AtomicBool,
}

impl Channel {
    #[inline]
    pub fn receiver(&self) -> Receiver {
        Receiver(self)
    }

    #[inline]
    pub fn sender(&self) -> Sender {
        Sender(self)
    }

    #[inline]
    pub fn prepared_sender(&self) -> PreparedSender {
        PreparedSender(self)
    }

    #[inline]
    pub fn start_recv(&self, token: &mut Token) -> bool {
        let mut step = 0;
        loop {
            if let Some(case) = self.wait_queues[0].remove_one() {
                unsafe {
                    if !case.is_prepared {
                        case.handle.inner.thread.unpark();

                        while case.handle.inner.request_ptr.load(Ordering::SeqCst) == 0 {
                        }

                        if case.handle.inner.request_ptr.load(Ordering::SeqCst) == 2 {
                            continue;
                        }
                    }

                    *token = Token::Case(mem::transmute::<Case, [usize; 3]>(case));
                    // TODO: wake up here to speed up?
                }
                return true;
            }

            if !self.is_closed() {
                return false;
            }

            step += 1;
            if step == 2 {
                *token = Token::Closed;
                return true;
            }
        }
    }

    pub fn fulfill_recv(&self, token: &mut Token) -> bool {
        // Wait until the requesting thread gives us a pointer to its `Request`.
        let handle = handle::current();

        let mut backoff = Backoff::new();
        loop {
            let ptr = handle.inner.request_ptr.load(Ordering::Acquire);
            if ptr == 2 {
                return false;
            }
            if ptr > 2 {
                break;
            }
            backoff.step();
        }

        *token = Token::Fulfill;
        true
    }

    pub unsafe fn read<T>(&self, token: &mut Token) -> Option<T> {
        match *token {
            Token::Closed => None,
            Token::Fulfill => {
                let req = HANDLE.with(|handle| {
                    let ptr = handle.inner.request_ptr.swap(0, Ordering::Acquire);
                    ptr as *const Request<Option<T>>
                });

                let m = unsafe {
                    // First, make a clone of the requesting thread.
                    let thread = (*req).handle.inner.thread.clone();

                    // Exchange the messages and then notify the requesting thread that it can pick up our
                    // message.
                    let m = (*req).exchange(None);
                    (*req).handle.try_select(CaseId::abort());

                    // Wake up the requesting thread.
                    thread.unpark();

                    // Return the exchanged message.
                    m
                };

                Some(m.unwrap())
            }
            Token::Case(case) => {
                let case: Case = mem::transmute::<[usize; 3], Case>(case);
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
                *token = Token::Case(mem::transmute::<Case, [usize; 3]>(case));
            }
            true
        } else {
            false
        }
    }

    pub unsafe fn write<T>(&self, token: &mut Token, msg: T, is_prepared: bool) {
        match *token {
            Token::Closed => unreachable!(),
            Token::Fulfill => {
                if !is_prepared {
                    let handle = handle::current();
                    handle.inner.request_ptr.store(1, Ordering::SeqCst);
                }
                fulfill(Some(msg));
            }
            Token::Case(ref case) => {
                let case: Case = mem::transmute::<[usize; 3], Case>(*case);
                finish_exchange(case, Some(msg));
            }
        }
        // TODO
    }

    pub fn fulfill_send(&self, token: &mut Token, is_prepared: bool) -> bool {
        *token = Token::Fulfill;
        true
    }

    pub unsafe fn finish_send(&self, token: &mut Token) {
        // TODO
    }

    pub unsafe fn fail_send(&self, token: &mut Token) {
        // TODO: wake up another sender? and make a test
        match *token {
            Token::Closed => unreachable!(),
            Token::Fulfill => {
                let handle = handle::current();
                handle.inner.request_ptr.store(2, Ordering::SeqCst);
            }
            Token::Case(ref case) => {
                let case: Case = mem::transmute::<[usize; 3], Case>(*case);
                case.handle.inner.request_ptr.store(2, Ordering::SeqCst);
            }
        }
    }

    /// Returns a new zero-capacity channel.
    #[inline]
    pub fn new() -> Self {
        Channel {
            wait_queues: [Waker::new(), Waker::new()],
            is_closed: AtomicBool::new(false),
        }
    }

    /// Returns a reference to the waker for this channel's senders.
    #[inline]
    fn senders(&self) -> &Waker {
        &self.wait_queues[0]
    }

    /// Returns a reference to the waker for this channel's receivers.
    #[inline]
    fn receivers(&self) -> &Waker {
        &self.wait_queues[1]
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
            let ptr = handle.inner.request_ptr.load(Ordering::Acquire);
            if ptr > 2 {
                handle.inner.request_ptr.store(0, Ordering::SeqCst);
                break ptr as *const Request<T>;
            }
            backoff.step();
        }
    });

    unsafe {
        // First, make a clone of the requesting thread.
        let thread = (*req).handle.inner.thread.clone();

        // Exchange the messages and then notify the requesting thread that it can pick up our
        // message.
        let m = (*req).exchange(msg);
        (*req).handle.try_select(CaseId::abort());

        // Wake up the requesting thread.
        thread.unpark();

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
