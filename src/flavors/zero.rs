//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::marker::PhantomData;

use parking_lot::Mutex;

use internal::select::{CaseId, Select, Token};
use internal::context;
use internal::utils::Backoff;
use internal::waker::{Case, Waker};

/// A zero-capacity channel.
pub struct Channel<T> {
    senders: Waker,
    receivers: Waker,
    is_closed: AtomicBool,
    lock: Mutex<()>,
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Constructs a new zero-capacity channel.
    pub fn new() -> Self {
        Channel {
            senders: Waker::new(),
            receivers: Waker::new(),
            is_closed: AtomicBool::new(false),
            lock: Mutex::new(()),
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
            *token = unsafe { ZeroToken::Case(mem::transmute::<Case, [usize; 3]>(case)) };
            true
        } else if self.is_closed() {
            // TODO: try recv again?
            *token = ZeroToken::Closed;
            true
        } else {
            false
        }
    }

    /// TODO
    fn fulfill_recv(&self, token: &mut Token) -> bool {
        let token = unsafe { &mut token.zero };

        let context = context::current();
        let mut backoff = Backoff::new();
        loop {
            let packet = context.packet.load(Ordering::SeqCst);
            if packet != 0 {
                *token = ZeroToken::Fulfill(packet);
                break;
            }
            backoff.step();
        }

        true
    }

    /// TODO
    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        let token = &mut token.zero;
        let packet;

        match token {
            ZeroToken::Closed => return None,
            ZeroToken::Fulfill(p) => {
                packet = *p as *const Packet<T>;
            }
            ZeroToken::Case(case) => {
                let case: Case = mem::transmute::<[usize; 3], Case>(*case);
                packet = case.packet as *const Packet<T>;
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
        let token = unsafe { &mut token.zero };

        // If there's someone on the other side, exchange message with it.
        if let Some(case) = self.receivers.wake_one() {
            unsafe {
                *token = ZeroToken::Case(mem::transmute::<Case, [usize; 3]>(case));
            }
            true
        } else {
            false
        }
    }

    /// TODO
    fn fulfill_send(&self, token: &mut Token) -> bool {
        let token = unsafe { &mut token.zero };

        let context = context::current();
        let mut backoff = Backoff::new();
        loop {
            let packet = context.packet.load(Ordering::SeqCst);
            if packet != 0 {
                *token = ZeroToken::Fulfill(packet);
                break;
            }
            backoff.step();
        }

        true
    }

    /// TODO
    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        let token = &mut token.zero;
        let packet;

        match token {
            ZeroToken::Closed => unreachable!(),
            ZeroToken::Fulfill(p) => {
                packet = *p as *const Packet<T>;
            }
            ZeroToken::Case(case) => {
                let case: Case = mem::transmute::<[usize; 3], Case>(*case);
                packet = case.packet as *const Packet<T>;
            }
        }

        *(*packet).msg.lock() = Some(msg);
        (*packet).ready.store(true, Ordering::Release);
    }

    pub fn send(&self, mut msg: T) {
        let mut token: Token = unsafe { ::std::mem::zeroed() }; // TODO: this is costly
        let case_id = CaseId::new(&token as *const Token as usize);
        let sender = self.sender();

        // TODO: maybe put a lock around wait queues?

        loop {
            let packet;
            {
                let guard = self.lock.lock();
                if sender.try(&mut token, &mut Backoff::new()) {
                    drop(guard);
                    unsafe { self.write(&mut token, msg); }
                    break;
                }

                context::current_reset();

                packet = Packet {
                    on_stack: true,
                    ready: AtomicBool::new(false),
                    msg: Mutex::new(Some(msg)),
                };
                self.senders.register_with_packet(case_id, &packet as *const _ as usize);

            }

            if !sender.is_blocked() {
                context::current_try_abort();
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
                self.senders.unregister(case_id);
                msg = packet.msg.into_inner().unwrap();
            }
        }
    }

    pub fn recv(&self) -> Option<T> {
        let mut token: Token = unsafe { ::std::mem::zeroed() }; // TODO: this is costly
        let case_id = CaseId::new(&token as *const Token as usize);
        let receiver = self.receiver();

        loop {
            let packet;
            {
                let guard = self.lock.lock();
                if receiver.try(&mut token, &mut Backoff::new()) {
                    drop(guard);
                    unsafe {
                        return self.read(&mut token);
                    }
                }

                context::current_reset();

                packet = Packet {
                    on_stack: true,
                    ready: AtomicBool::new(false),
                    msg: Mutex::new(None::<T>),
                };
                self.receivers.register_with_packet(case_id, &packet as *const _ as usize);
            }

            if !receiver.is_blocked() {
                context::current_try_abort();
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
                self.receivers.unregister(case_id);
            }
        }
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

struct Packet<T> {
    on_stack: bool,
    ready: AtomicBool,
    msg: Mutex<Option<T>>,
}

#[derive(Copy, Clone)]
pub enum ZeroToken {
    Closed,
    Fulfill(usize),
    Case([usize; 3]), // TODO: use [u8; mem::size_of::<Case>()], write and read unaligned
}

pub struct Receiver<'a, T: 'a>(&'a Channel<T>);
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> Select for Receiver<'a, T> {
    fn try(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        self.0.start_recv(token)
    }

    fn promise(&self, _token: &mut Token, case_id: CaseId) {
        let packet = Box::into_raw(Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: Mutex::new(None::<T>),
        }));
        self.0.receivers.register_with_packet(case_id, packet as usize);
    }

    fn is_blocked(&self) -> bool {
        !self.0.senders.can_notify() && !self.0.is_closed()
    }

    fn revoke(&self, case_id: CaseId) {
        if let Some(case) = self.0.receivers.unregister(case_id) {
            unsafe {
                drop(Box::from_raw(case.packet as *mut Packet<T>));
            }
        }
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
        let packet = Box::into_raw(Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: Mutex::new(None::<T>),
        }));
        self.0.senders.register_with_packet(case_id, packet as usize);
    }

    fn is_blocked(&self) -> bool {
        !self.0.receivers.can_notify()
    }

    fn revoke(&self, case_id: CaseId) {
        if let Some(case) = self.0.senders.unregister(case_id) {
            unsafe {
                drop(Box::from_raw(case.packet as *mut Packet<T>));
            }
        }
    }

    fn fulfill(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        self.0.fulfill_send(token)
    }
}
