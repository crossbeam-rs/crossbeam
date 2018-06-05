//! A zero-capacity channel, or sometimes called *rendezvous* channel.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use internal::channel::RecvNonblocking;
use internal::context;
use internal::select::{CaseId, Select, Token};
use internal::utils::Backoff;
use internal::waker::Waker;

// TODO: anything that calls read/write in any flavor should have an abort guard

struct Inner {
    senders: Waker,
    receivers: Waker,
    is_closed: bool,
}

/// A zero-capacity channel.
pub struct Channel<T> {
    inner: Mutex<Inner>,
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Constructs a new zero-capacity channel.
    pub fn new() -> Self {
        Channel {
            inner: Mutex::new(Inner {
                senders: Waker::new(),
                receivers: Waker::new(),
                is_closed: false,
            }),
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
        let mut inner = self.inner.lock();

        if let Some(case) = inner.senders.wake_one() {
            token.zero = Some(case.packet);
            true
        } else if inner.is_closed {
            token.zero = None;
            true
        } else {
            false
        }
    }

    /// TODO
    fn accept_recv(&self, token: &mut Token) -> bool {
        let context = context::current();
        let mut backoff = Backoff::new();
        loop {
            let packet = context.packet.load(Ordering::Acquire);
            if packet != 0 {
                token.zero = Some(packet);
                break;
            }
            backoff.step();
        }

        true
    }

    /// TODO
    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        let packet;

        match token.zero {
            None => return None,
            Some(p) => {
                packet = p as *const Packet<T>;
            }
        }

        if (*packet).on_stack {
            let msg = (*packet).msg.lock().take();
            (*packet).ready.store(true, Ordering::Release);
            msg
        } else {
            let mut backoff = Backoff::new();
            while !(*packet).ready.load(Ordering::Acquire) {
                backoff.step();
            }
            let msg = (*packet).msg.lock().take();
            drop(Box::from_raw(packet as *mut Packet<T>));
            msg
        }
    }

    /// TODO
    fn start_send(&self, token: &mut Token) -> bool {
        let mut inner = self.inner.lock();

        // If there's someone on the other side, exchange message with it.
        if let Some(case) = inner.receivers.wake_one() {
            token.zero = Some(case.packet);
            true
        } else {
            false
        }
    }

    /// TODO
    fn accept_send(&self, token: &mut Token) -> bool {
        let context = context::current();
        let mut backoff = Backoff::new();
        loop {
            let packet = context.packet.load(Ordering::Acquire);
            if packet != 0 {
                token.zero = Some(packet);
                break;
            }
            backoff.step();
        }

        true
    }

    /// TODO
    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        let packet = token.zero.unwrap() as *const Packet<T>;

        *(*packet).msg.lock() = Some(msg);
        (*packet).ready.store(true, Ordering::Release);
    }

    pub fn send(&self, mut msg: T) {
        let token = &mut Token::default();
        let case_id = CaseId::new(token as *mut Token as usize);

        loop {
            let packet;
            {
                let mut inner = self.inner.lock();
                // If there's someone on the other side, exchange message with it.
                if let Some(case) = inner.receivers.wake_one() {
                    token.zero = Some(case.packet);
                    drop(inner);
                    unsafe { self.write(token, msg); }
                    break;
                }

                context::current_reset();

                packet = Packet {
                    on_stack: true,
                    ready: AtomicBool::new(false),
                    msg: Mutex::new(Some(msg)),
                };
                inner.senders.register_with_packet(case_id, &packet as *const _ as usize);
            }

            context::current_wait_until(None);

            let s = context::current_selected();
            if s == case_id {
                let mut backoff = Backoff::new();
                while !packet.ready.load(Ordering::Acquire) {
                    backoff.step();
                }
                break;
            } else {
                self.inner.lock().senders.unregister(case_id);
                msg = packet.msg.into_inner().unwrap();
            }
        }
    }

    pub fn recv(&self) -> Option<T> {
        let token = &mut Token::default();
        let case_id = CaseId::new(token as *mut Token as usize);

        loop {
            let packet;
            {
                let mut inner = self.inner.lock();

                if let Some(case) = inner.senders.wake_one() {
                    token.zero = Some(case.packet);
                    drop(inner);
                    unsafe {
                        return self.read(token);
                    }
                }

                if inner.is_closed {
                    return None;
                }

                context::current_reset();

                packet = Packet {
                    on_stack: true,
                    ready: AtomicBool::new(false),
                    msg: Mutex::new(None::<T>),
                };
                inner.receivers.register_with_packet(case_id, &packet as *const _ as usize);
            }

            context::current_wait_until(None);

            let s = context::current_selected();
            if s == case_id {
                let mut backoff = Backoff::new();
                while !packet.ready.load(Ordering::Acquire) {
                    backoff.step();
                }
                return Some(packet.msg.into_inner().unwrap());
            } else {
                self.inner.lock().receivers.unregister(case_id);
            }
        }
    }

    pub fn recv_nonblocking(&self) -> RecvNonblocking<T> {
        let token = &mut Token::default();

        let mut inner = self.inner.lock();

        if let Some(case) = inner.senders.wake_one() {
            token.zero = Some(case.packet);
            drop(inner);

            match unsafe { self.read(token) } {
                None => RecvNonblocking::Closed,
                Some(msg) => RecvNonblocking::Message(msg),
            }
        } else if inner.is_closed {
            RecvNonblocking::Closed
        } else {
            RecvNonblocking::Empty
        }
    }

    /// Closes the channel and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        let mut inner = self.inner.lock();

        if inner.is_closed {
            false
        } else {
            inner.is_closed = true;
            inner.receivers.abort_all();
            true
        }
    }

    /// Returns the current number of messages inside the channel.
    pub fn len(&self) -> usize {
        0
    }

    /// Returns the capacity of the channel.
    pub fn capacity(&self) -> Option<usize> {
        Some(0)
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        true
    }

    /// Returns `true` if the channel is full.
    pub fn is_full(&self) -> bool {
        true
    }
}

struct Packet<T> {
    on_stack: bool,
    ready: AtomicBool,
    msg: Mutex<Option<T>>,
}

pub type ZeroToken = Option<usize>;

pub struct Receiver<'a, T: 'a>(&'a Channel<T>);
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> Select for Receiver<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_recv(token)
    }

    fn retry(&self, token: &mut Token) -> bool {
        // self.0.start_recv(token)

        let case_id = CaseId::new(&token as *const _ as usize);
        let mut inner = self.0.inner.lock();

        if let Some(case) = inner.senders.wake_one() {
            token.zero = Some(case.packet);
            return true;
        } else if inner.is_closed {
            token.zero = None;
            return true;
        }

        context::current_reset();

        let packet = Box::into_raw(Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: Mutex::new(None::<T>),
        }));
        inner.receivers.register_with_packet(case_id, packet as usize);

        drop(inner);

        thread::yield_now();
        context::current_try_abort();

        if context::current_selected() != CaseId::abort() {
            let context = context::current();
            let mut backoff = Backoff::new();
            loop {
                let packet = context.packet.load(Ordering::Acquire);
                if packet != 0 {
                    token.zero = Some(packet);
                    break;
                }
                backoff.step();
            }

            true
        } else {
            if let Some(case) = self.0.inner.lock().receivers.unregister(case_id) {
                unsafe {
                    drop(Box::from_raw(case.packet as *mut Packet<T>));
                }
            }
            false
        }
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, case_id: CaseId) -> bool {
        let packet = Box::into_raw(Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: Mutex::new(None::<T>),
        }));

        let mut inner = self.0.inner.lock();
        inner.receivers.register_with_packet(case_id, packet as usize);
        !inner.senders.can_notify() && !inner.is_closed
    }

    fn unregister(&self, case_id: CaseId) {
        if let Some(case) = self.0.inner.lock().receivers.unregister(case_id) {
            unsafe {
                drop(Box::from_raw(case.packet as *mut Packet<T>));
            }
        }
    }

    fn accept(&self, token: &mut Token) -> bool {
        self.0.accept_recv(token)
    }
}

impl<'a, T> Select for Sender<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_send(token)
    }

    fn retry(&self, token: &mut Token) -> bool {
        // self.0.start_send(token)

        let case_id = CaseId::new(&token as *const _ as usize);
        let mut inner = self.0.inner.lock();

        // If there's someone on the other side, exchange message with it.
        if let Some(case) = inner.receivers.wake_one() {
            token.zero = Some(case.packet);
            return true;
        }

        context::current_reset();

        let packet = Box::into_raw(Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: Mutex::new(None::<T>),
        }));
        inner.senders.register_with_packet(case_id, packet as usize);

        drop(inner);

        thread::yield_now();
        context::current_try_abort();

        if context::current_selected() != CaseId::abort() {
            let context = context::current();
            let mut backoff = Backoff::new();
            loop {
                let packet = context.packet.load(Ordering::Acquire);
                if packet != 0 {
                    token.zero = Some(packet);
                    break;
                }
                backoff.step();
            }

            true
        } else {
            if let Some(case) = self.0.inner.lock().senders.unregister(case_id) {
                unsafe {
                    drop(Box::from_raw(case.packet as *mut Packet<T>));
                }
            }
            false
        }
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, case_id: CaseId) -> bool {
        let packet = Box::into_raw(Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: Mutex::new(None::<T>),
        }));

        let mut inner = self.0.inner.lock();
        inner.senders.register_with_packet(case_id, packet as usize);
        !inner.receivers.can_notify()
    }

    fn unregister(&self, case_id: CaseId) {
        if let Some(case) = self.0.inner.lock().senders.unregister(case_id) {
            unsafe {
                drop(Box::from_raw(case.packet as *mut Packet<T>));
            }
        }
    }

    fn accept(&self, token: &mut Token) -> bool {
        self.0.accept_send(token)
    }
}
