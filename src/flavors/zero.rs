//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::marker::PhantomData;

use parking_lot::Mutex;

use internal::select::{CaseId, Select, Token};
use internal::context::{self, CONTEXT, Context};
use internal::utils::Backoff;
use internal::waker::{Case, Waker};

/// A zero-capacity channel.
pub struct Channel<T> {
    senders: Waker,
    receivers: Waker,
    is_closed: AtomicBool,
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Constructs a new zero-capacity channel.
    pub fn new() -> Self {
        Channel {
            senders: Waker::new(),
            receivers: Waker::new(),
            is_closed: AtomicBool::new(false),
            _marker: PhantomData,
        }
    }

    /// Returns a receiver handle to the channel.
    pub fn receiver(&self) -> Receiver<T> {
        Receiver(self)
    }

    /// Returns a sender handle to the channel.
    pub fn sender(&self) -> Sender<T> {
        Sender(self)
    }

    /// TODO
    fn start_recv(&self, token: &mut Token) -> bool {
        let token = unsafe { &mut token.zero };

        if let Some(case) = self.senders.wake_one() {
            *token = unsafe { ZeroToken::Case(mem::transmute::<Case, [usize; 2]>(case)) };
            true
        } else if self.is_closed() {
            *token = ZeroToken::Closed;
            true
        } else {
            false
        }
    }

    /// TODO
    fn fulfill_recv(&self, token: &mut Token) -> bool {
        let token = unsafe { &mut token.zero };

        // Wait until the requesting thread gives us a pointer to its `Request`.
        let context = context::current();

        let mut backoff = Backoff::new();
        while context.request_ptr.load(Ordering::Acquire) == 0 {
            backoff.step();
        }

        *token = ZeroToken::Fulfill;
        true
    }

    /// TODO
    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        let token = &mut token.zero;

        match token {
            ZeroToken::Closed => None,
            ZeroToken::Fulfill => {
                let req = CONTEXT.with(|context| {
                    let ptr = context.request_ptr.swap(0, Ordering::Acquire);
                    ptr as *const Request<Option<T>>
                });

                let m = {
                    // First, make a clone of the requesting thread.
                    let thread = (*req).context.thread.clone();

                    // Exchange the messages and then notify the requesting thread that it can pick up our
                    // message.
                    let m = (*req).exchange(None);
                    (*req).context.try_select(CaseId::abort());

                    // Wake up the requesting thread.
                    thread.unpark();

                    // Return the exchanged message.
                    m
                };

                Some(m.unwrap())
            }
            ZeroToken::Case(case) => {
                let case: Case = mem::transmute::<[usize; 2], Case>(*case);
                Some(finish_exchange(case, None).unwrap())
            }
        }
        // TODO
    }

    /// TODO
    fn start_send(&self, token: &mut Token) -> bool {
        let token = unsafe { &mut token.zero };

        // If there's someone on the other side, exchange messages with it.
        if let Some(case) = self.receivers.wake_one() {
            unsafe {
                *token = ZeroToken::Case(mem::transmute::<Case, [usize; 2]>(case));
            }
            true
        } else {
            false
        }
    }

    /// TODO
    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        let token = &mut token.zero;

        match token {
            ZeroToken::Closed => unreachable!(),
            ZeroToken::Fulfill => {
                fulfill(Some(msg));
            }
            ZeroToken::Case(case) => {
                let case: Case = mem::transmute::<[usize; 2], Case>(*case);
                finish_exchange(case, Some(msg));
            }
        }
        // TODO
    }

    /// TODO
    fn fulfill_send(&self, token: &mut Token) -> bool {
        let token = unsafe { &mut token.zero };
        *token = ZeroToken::Fulfill;
        true
    }

    /// Closes the exchanger and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        if !self.is_closed.swap(true, Ordering::SeqCst) {
            self.senders.abort_all();
            self.receivers.abort_all();
            true
        } else {
            false
        }
    }

    /// Returns `true` if the exchanger is closed.
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::SeqCst)
    }
}

/// TODO
unsafe fn finish_exchange<T>(case: Case, msg: T) -> T {
    // This is a promise.
    // We must request the message and then wait until the promise is fulfilled.

    // Reset the current thread's selection case.
    context::current_reset();

    // Create a request on the stack and register it in the owner of this case.
    let req = Request::new(msg);
    case.context.request_ptr.store(&req as *const _ as usize, Ordering::Release);

    // Wake up the owner of this case.
    case.context.thread.unpark();

    // Wait until our selection case is woken.
    context::current_wait_until(None);

    // Extract the received message from the request.
    req.into_msg()
}

/// Fulfills the previously made promise.
fn fulfill<T>(msg: T) -> T {
    // Wait until the requesting thread gives us a pointer to its `Request`.
    let req = CONTEXT.with(|context| {
        let mut backoff = Backoff::new();
        loop {
            let ptr = context.request_ptr.load(Ordering::Acquire);
            if ptr != 0 {
                context.request_ptr.store(0, Ordering::SeqCst);
                break ptr as *const Request<T>;
            }
            backoff.step();
        }
    });

    unsafe {
        // First, make a clone of the requesting thread.
        let thread = (*req).context.thread.clone();

        // Exchange the messages and then notify the requesting thread that it can pick up our
        // message.
        let m = (*req).exchange(msg);
        (*req).context.try_select(CaseId::abort());

        // Wake up the requesting thread.
        thread.unpark();

        // Return the exchanged message.
        m
    }
}

/// A request for promised message.
struct Request<T> {
    /// The context associated with the requestor.
    context: Arc<Context>,

    /// The message for exchange.
    msg: Mutex<Option<T>>,
}

impl<T> Request<T> {
    /// Creates a new request owned by the current thread for exchanging `msg`.
    fn new(msg: T) -> Self {
        Request {
            context: context::current(),
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

#[derive(Copy, Clone)]
pub enum ZeroToken {
    Closed,
    Fulfill,
    Case([usize; 2]), // TODO: use [u8; mem::size_of::<Case>()], write and read unaligned
}

pub struct Receiver<'a, T: 'a>(&'a Channel<T>);
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> Select for Receiver<'a, T> {
    fn try(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        self.0.start_recv(token)
    }

    fn promise(&self, _token: &mut Token, case_id: CaseId) {
        self.0.receivers.register(case_id)
    }

    fn is_blocked(&self) -> bool {
        !self.0.senders.can_notify() && !self.0.is_closed()
    }

    fn revoke(&self, case_id: CaseId) {
        self.0.receivers.unregister(case_id);
    }

    fn fulfill(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        self.0.fulfill_recv(token)
    }
}

impl<'a, T> Select for Sender<'a, T> {
    fn try(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        self.0.start_send(token)
    }

    fn promise(&self, _token: &mut Token, case_id: CaseId) {
        self.0.senders.register(case_id)
    }

    fn is_blocked(&self) -> bool {
        !self.0.receivers.can_notify()
    }

    fn revoke(&self, case_id: CaseId) {
        self.0.senders.unregister(case_id);
    }

    fn fulfill(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        self.0.fulfill_send(token)
    }
}
