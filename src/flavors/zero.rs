//! Zero-capacity channel.
//!
//! This kind of channel also known as *rendezvous* channel.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
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

/// Pointer to a packet.
pub type ZeroToken = usize;

/// Slot for passing one message from a sender to a receiver.
struct Packet<T> {
    /// Equals `true` if the packet is allocated on the stack.
    on_stack: bool,

    /// Equals `true` once the packet is ready for reading or writing.
    ready: AtomicBool,

    /// A message.
    msg: ManuallyDrop<UnsafeCell<T>>,
}

impl<T> Packet<T> {
    /// Creates an empty packet on the stack.
    fn empty_on_stack() -> Packet<T> {
        Packet {
            on_stack: true,
            ready: AtomicBool::new(false),
            msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
        }
    }

    /// Creates an empty packet on the heap.
    fn empty_on_heap() -> Box<Packet<T>> {
        Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: unsafe { ManuallyDrop::new(UnsafeCell::new(mem::uninitialized())) },
        })
    }

    /// Creates an packet on the heap, containing a message.
    fn message_on_stack(msg: T) -> Packet<T> {
        Packet {
            on_stack: true,
            ready: AtomicBool::new(false),
            msg: ManuallyDrop::new(UnsafeCell::new(msg)),
        }
    }

    /// Waits until the packet becomes ready for reading or writing.
    fn wait_ready(&self) {
        let backoff = &mut Backoff::new();
        while !self.ready.load(Ordering::Acquire) {
            backoff.step();
        }
    }
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

/// Zero-capacity channel.
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
    fn start_send(&self, token: &mut Token, short_pause: bool) -> bool {
        let case_id = CaseId::hook(token);

        {
            let mut inner = self.inner.lock();

            // If there's a waiting receiver, pair up with it.
            if let Some(case) = inner.receivers.wake_one() {
                token.zero = case.packet;
                return true;
            }

            if !short_pause {
                return false;
            }

            // Register this send operation so that another receiver can pair up with it.
            context::current_reset();
            let packet = Box::into_raw(Packet::<T>::empty_on_heap());
            inner.senders.register_with_packet(case_id, packet as usize);
        }

        // Yield to give receivers a chance to pair up with this operation.
        thread::yield_now();
        let sel = context::current_try_abort();

        match sel {
            CaseId::Waiting | CaseId::Closed => unreachable!(),
            CaseId::Aborted => {
                // Unregister and destroy the packet.
                let case = self.inner.lock().senders.unregister(case_id).unwrap();
                unsafe {
                    drop(Box::from_raw(case.packet as *mut Packet<T>));
                }
                false
            },
            CaseId::Case(_) => {
                // Success! A receiver has paired up with this operation.
                token.zero = context::current_wait_packet();
                true
            },
        }
    }

    /// Writes a message into the packet.
    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        let packet = &*(token.zero as *const Packet<T>);
        packet.msg.get().write(msg);
        packet.ready.store(true, Ordering::Release);
    }

    /// Attempts to pair up with a sender.
    fn start_recv(&self, token: &mut Token, short_pause: bool) -> bool {
        let case_id = CaseId::hook(token);

        {
            let mut inner = self.inner.lock();

            // If there's a waiting sender, pair up with it.
            if let Some(case) = inner.senders.wake_one() {
                token.zero = case.packet;
                return true;
            } else if inner.is_closed {
                token.zero = 0;
                return true;
            }

            if !short_pause {
                return false;
            }

            // Register this receive operation so that another sender can pair up with it.
            context::current_reset();
            let packet = Box::into_raw(Packet::<T>::empty_on_heap());
            inner.receivers.register_with_packet(case_id, packet as usize);
        }

        // Yield to give senders a chance to pair up with this operation.
        thread::yield_now();
        let sel = context::current_try_abort();

        match sel {
            CaseId::Waiting => unreachable!(),
            CaseId::Aborted => {
                // Unregister and destroy the packet.
                let case = self.inner.lock().receivers.unregister(case_id).unwrap();
                unsafe {
                    drop(Box::from_raw(case.packet as *mut Packet<T>));
                }
                false
            },
            CaseId::Closed => {
                // Unregister and destroy the packet.
                let case = self.inner.lock().receivers.unregister(case_id).unwrap();
                unsafe {
                    drop(Box::from_raw(case.packet as *mut Packet<T>));
                }

                // All senders have just been dropped.
                token.zero = 0;
                true
            },
            CaseId::Case(_) => {
                // Success! A sender has paired up with this operation.
                token.zero = context::current_wait_packet();
                true
            },
        }
    }

    /// Reads a message from the packet.
    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        // If there is no packet, the channel is closed.
        if token.zero == 0 {
            return None;
        }

        let packet = &*(token.zero as *const Packet<T>);

        if packet.on_stack {
            // The message was there since the packet was constructed. After reading the message,
            // we need to set `ready` to `true` in order to signal that the packet can be
            // destroyed.
            let msg = packet.msg.get().read();
            packet.ready.store(true, Ordering::Release);
            Some(msg)
        } else {
            // Wait until the message becomes available, then read it and destroy the
            // heap-allocated packet.
            packet.wait_ready();
            let msg = packet.msg.get().read();
            drop(Box::from_raw(packet as *const Packet<T> as *mut Packet<T>));
            Some(msg)
        }
    }

    /// Sends a message into the channel.
    pub fn send(&self, msg: T) {
        let token = &mut Token::default();
        let case_id = CaseId::hook(token);

        let packet;
        {
            let mut inner = self.inner.lock();

            // If there's a waiting receiver, pair up with it.
            if let Some(case) = inner.receivers.wake_one() {
                token.zero = case.packet;
                drop(inner);
                unsafe { self.write(token, msg); }
                return;
            }

            // Prepare for blocking until a receiver wakes us up.
            context::current_reset();
            packet = Packet::message_on_stack(msg);
            inner.senders.register_with_packet(
                case_id,
                &packet as *const Packet<T> as usize,
            );
        }

        // Block the current thread.
        let sel = context::current_wait_until(None);

        match sel {
            CaseId::Waiting | CaseId::Aborted | CaseId::Closed => unreachable!(),
            CaseId::Case(_) => {
                // Wait until the message is read, then drop the packet.
                packet.wait_ready();
            },
        }
    }

    /// Receives a message from the channel.
    pub fn recv(&self) -> Option<T> {
        let token = &mut Token::default();
        let case_id = CaseId::hook(token);

        let packet;
        {
            let mut inner = self.inner.lock();

            // If there's a waiting sender, pair up with it.
            if let Some(case) = inner.senders.wake_one() {
                token.zero = case.packet;
                drop(inner);
                unsafe { return self.read(token); }
            }

            if inner.is_closed {
                return None;
            }

            // Prepare for blocking until a sender wakes us up.
            context::current_reset();
            packet = Packet::<T>::empty_on_stack();
            inner.receivers.register_with_packet(
                case_id,
                &packet as *const Packet<T> as usize,
            );
        }

        // Block the current thread.
        let sel = context::current_wait_until(None);

        match sel {
            CaseId::Waiting | CaseId::Aborted => unreachable!(),
            CaseId::Closed => {
                self.inner.lock().receivers.unregister(case_id).unwrap();
                None
            },
            CaseId::Case(_) => {
                // Wait until the message is provided, then read it.
                packet.wait_ready();
                unsafe { Some(packet.msg.get().read()) }
            },
        }
    }

    /// Attempts to receive a message without blocking.
    pub fn recv_nonblocking(&self) -> RecvNonblocking<T> {
        let token = &mut Token::default();
        let mut inner = self.inner.lock();

        // If there's a waiting sender, pair up with it.
        if let Some(case) = inner.senders.wake_one() {
            token.zero = case.packet;
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

    /// Closes the channel and wakes up all blocked receivers.
    pub fn close(&self) -> bool {
        let mut inner = self.inner.lock();

        if inner.is_closed {
            false
        } else {
            inner.is_closed = true;
            inner.receivers.close();
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

/// Receiver handle to a channel.
pub struct Receiver<'a, T: 'a>(&'a Channel<T>);

/// Sender handle to a channel.
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> Select for Receiver<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_recv(token, false)
    }

    fn retry(&self, token: &mut Token) -> bool {
        self.0.start_recv(token, true)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, case_id: CaseId) -> bool {
        let packet = Box::into_raw(Packet::<T>::empty_on_heap());

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
        token.zero = context::current_wait_packet();
        true
    }
}

impl<'a, T> Select for Sender<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_send(token, false)
    }

    fn retry(&self, token: &mut Token) -> bool {
        self.0.start_send(token, true)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, case_id: CaseId) -> bool {
        let packet = Box::into_raw(Packet::<T>::empty_on_heap());

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
        token.zero = context::current_wait_packet();
        true
    }
}
