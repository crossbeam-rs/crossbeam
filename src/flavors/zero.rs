//! A zero-capacity channel.
//!
//! This kind of channel also known as *rendezvous* channel.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
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

/// Result of a receive operation.
pub type ZeroToken = Option<usize>;

/// TODO
struct Packet<T> {
    on_stack: bool,
    ready: AtomicBool,
    msg: ManuallyDrop<UnsafeCell<T>>,
}

/// Inner representation of a zero-capacity channel.
struct Inner {
    /// Senders waiting to pair up with a receive operation.
    senders: Waker,

    /// Receivers waiting to pair up with a send operation.
    receivers: Waker,

    /// Equals `true` when the channel is closed.
    is_closed: bool,
}

/// A zero-capacity channel.
pub struct Channel<T> {
    /// Inner representation of the channel.
    inner: Mutex<Inner>,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
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

    /// Attempts to reserve a slot for sending a message.
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
    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        let packet = token.zero.unwrap() as *const Packet<T>;

        ptr::write((*packet).msg.get(), msg);
        (*packet).ready.store(true, Ordering::Release);
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
    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        let packet;

        match token.zero {
            None => return None,
            Some(p) => {
                packet = p as *const Packet<T>;
            }
        }

        if (*packet).on_stack {
            let msg = ptr::read((*packet).msg.get());
            (*packet).ready.store(true, Ordering::Release);
            Some(msg)
        } else {
            let mut backoff = Backoff::new();
            while !(*packet).ready.load(Ordering::Acquire) {
                backoff.step();
            }
            let msg = ptr::read((*packet).msg.get());
            drop(Box::from_raw(packet as *mut Packet<T>));
            Some(msg)
        }
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
                    msg: ManuallyDrop::new(UnsafeCell::new(msg)),
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
                msg = unsafe { ptr::read(packet.msg.get()) };
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

                packet = Packet::<T> {
                    on_stack: true,
                    ready: AtomicBool::new(false),
                    msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
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
                let msg = unsafe { ptr::read(packet.msg.get()) };
                return Some(msg);
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

/// A receiver handle to a channel.
pub struct Receiver<'a, T: 'a>(&'a Channel<T>);

/// A sender handle to a channel.
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

        let packet = Box::into_raw(Box::new(Packet::<T> {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
        }));
        inner.receivers.register_with_packet(case_id, packet as usize);

        drop(inner);

        thread::yield_now();
        context::current_try_abort();

        if context::current_selected() != CaseId::abort() {
            token.zero = Some(context::current_wait_packet());
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
        let packet = Box::into_raw(Box::new(Packet::<T> {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
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
        token.zero = Some(context::current_wait_packet());
        true
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

        let packet = Box::into_raw(Box::new(Packet::<T> {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
        }));
        inner.senders.register_with_packet(case_id, packet as usize);

        drop(inner);

        thread::yield_now();
        context::current_try_abort();

        if context::current_selected() != CaseId::abort() {
            token.zero = Some(context::current_wait_packet());
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
        let packet = Box::into_raw(Box::new(Packet::<T> {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
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
        token.zero = Some(context::current_wait_packet());
        true
    }
}
