//! Zero-capacity channel.
//!
//! This kind of channel is also known as *rendezvous* channel.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use context::Context;
use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use select::{Operation, SelectHandle, Selected, Token};
use utils::Backoff;
use waker::Waker;

/// A pointer to a packet.
pub type ZeroToken = usize;

/// A slot for passing one message from a sender to a receiver.
struct Packet<T> {
    /// Equals `true` if the packet is allocated on the stack.
    on_stack: bool,

    /// Equals `true` once the packet is ready for reading or writing.
    ready: AtomicBool,

    /// The message.
    msg: UnsafeCell<Option<T>>,
}

impl<T> Packet<T> {
    /// Creates an empty packet on the stack.
    fn empty_on_stack() -> Packet<T> {
        Packet {
            on_stack: true,
            ready: AtomicBool::new(false),
            msg: UnsafeCell::new(None),
        }
    }

    /// Creates an empty packet on the heap.
    fn empty_on_heap() -> Box<Packet<T>> {
        Box::new(Packet {
            on_stack: false,
            ready: AtomicBool::new(false),
            msg: UnsafeCell::new(None),
        })
    }

    /// Creates a packet on the stack, containing a message.
    fn message_on_stack(msg: T) -> Packet<T> {
        Packet {
            on_stack: true,
            ready: AtomicBool::new(false),
            msg: UnsafeCell::new(Some(msg)),
        }
    }

    /// Waits until the packet becomes ready for reading or writing.
    fn wait_ready(&self) {
        let mut backoff = Backoff::new();
        while !self.ready.load(Ordering::Acquire) {
            backoff.snooze();
        }
    }
}

/// Inner representation of a zero-capacity channel.
struct Inner {
    /// Senders waiting to pair up with a receive operation.
    senders: Waker,

    /// Receivers waiting to pair up with a send operation.
    receivers: Waker,

    /// Equals `true` when the channel is disconnected.
    is_disconnected: bool,
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
                is_disconnected: false,
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
        let mut inner = self.inner.lock();

        // If there's a waiting receiver, pair up with it.
        if let Some(operation) = inner.receivers.wake_one() {
            token.zero = operation.packet;
            return true;
        } else if inner.is_disconnected {
            token.zero = 0;
            return true;
        }

        if !short_pause {
            return false;
        }

        Context::with(|cx| {
            // Register this send operation so that another receiver can pair up with it.
            let oper = Operation::hook(token);
            let packet = Box::into_raw(Packet::<T>::empty_on_heap());
            inner
                .senders
                .register_with_packet(oper, packet as usize, cx);
            drop(inner);

            // Yield to give receivers a chance to pair up with this operation.
            thread::yield_now();

            let sel = match cx.try_select(Selected::Aborted) {
                Ok(()) => Selected::Aborted,
                Err(s) => s,
            };

            match sel {
                Selected::Waiting => unreachable!(),
                Selected::Disconnected => {
                    // Unregister and destroy the packet.
                    let operation = self.inner.lock().senders.unregister(oper).unwrap();
                    unsafe {
                        drop(Box::from_raw(operation.packet as *mut Packet<T>));
                    }

                    // All receivers have just been dropped.
                    token.zero = 0;
                    true
                }
                Selected::Aborted => {
                    // Unregister and destroy the packet.
                    let operation = self.inner.lock().senders.unregister(oper).unwrap();
                    unsafe {
                        drop(Box::from_raw(operation.packet as *mut Packet<T>));
                    }
                    false
                }
                Selected::Operation(_) => {
                    // Success! A receiver has paired up with this operation.
                    token.zero = cx.wait_packet();
                    true
                }
            }
        })
    }

    /// Writes a message into the packet.
    pub unsafe fn write(&self, token: &mut Token, msg: T) -> Result<(), T> {
        // If there is no packet, the channel is disconnected.
        if token.zero == 0 {
            return Err(msg);
        }

        let packet = &*(token.zero as *const Packet<T>);
        packet.msg.get().write(Some(msg));
        packet.ready.store(true, Ordering::Release);
        Ok(())
    }

    /// Attempts to pair up with a sender.
    fn start_recv(&self, token: &mut Token, short_pause: bool) -> bool {
        let mut inner = self.inner.lock();

        // If there's a waiting sender, pair up with it.
        if let Some(operation) = inner.senders.wake_one() {
            token.zero = operation.packet;
            return true;
        } else if inner.is_disconnected {
            token.zero = 0;
            return true;
        }

        if !short_pause {
            return false;
        }

        Context::with(|cx| {
            // Register this receive operation so that another sender can pair up with it.
            let oper = Operation::hook(token);
            let packet = Box::into_raw(Packet::<T>::empty_on_heap());
            inner
                .receivers
                .register_with_packet(oper, packet as usize, cx);
            drop(inner);

            // Yield to give senders a chance to pair up with this operation.
            thread::yield_now();

            let sel = match cx.try_select(Selected::Aborted) {
                Ok(()) => Selected::Aborted,
                Err(s) => s,
            };

            match sel {
                Selected::Waiting => unreachable!(),
                Selected::Aborted => {
                    // Unregister and destroy the packet.
                    let operation = self.inner.lock().receivers.unregister(oper).unwrap();
                    unsafe {
                        drop(Box::from_raw(operation.packet as *mut Packet<T>));
                    }
                    false
                }
                Selected::Disconnected => {
                    // Unregister and destroy the packet.
                    let operation = self.inner.lock().receivers.unregister(oper).unwrap();
                    unsafe {
                        drop(Box::from_raw(operation.packet as *mut Packet<T>));
                    }

                    // All senders have just been dropped.
                    token.zero = 0;
                    true
                }
                Selected::Operation(_) => {
                    // Success! A sender has paired up with this operation.
                    token.zero = cx.wait_packet();
                    true
                }
            }
        })
    }

    /// Reads a message from the packet.
    pub unsafe fn read(&self, token: &mut Token) -> Result<T, ()> {
        // If there is no packet, the channel is disconnected.
        if token.zero == 0 {
            return Err(());
        }

        let packet = &*(token.zero as *const Packet<T>);

        if packet.on_stack {
            // The message has been in the packet from the beginning, so there is no need to wait
            // for it. However, after reading the message, we need to set `ready` to `true` in
            // order to signal that the packet can be destroyed.
            let msg = packet.msg.get().replace(None).unwrap();
            packet.ready.store(true, Ordering::Release);
            Ok(msg)
        } else {
            // Wait until the message becomes available, then read it and destroy the
            // heap-allocated packet.
            packet.wait_ready();
            let msg = packet.msg.get().replace(None).unwrap();
            drop(Box::from_raw(packet as *const Packet<T> as *mut Packet<T>));
            Ok(msg)
        }
    }

    /// Attempts to send a message into the channel.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        let token = &mut Token::default();
        let mut inner = self.inner.lock();

        // If there's a waiting receiver, pair up with it.
        if let Some(operation) = inner.receivers.wake_one() {
            token.zero = operation.packet;
            drop(inner);
            unsafe {
                self.write(token, msg).ok().unwrap();
            }
            Ok(())
        } else if inner.is_disconnected {
            Err(TrySendError::Disconnected(msg))
        } else {
            Err(TrySendError::Full(msg))
        }
    }

    /// Sends a message into the channel.
    pub fn send(&self, msg: T, deadline: Option<Instant>) -> Result<(), SendTimeoutError<T>> {
        let token = &mut Token::default();
        let mut inner = self.inner.lock();

        // If there's a waiting receiver, pair up with it.
        if let Some(operation) = inner.receivers.wake_one() {
            token.zero = operation.packet;
            drop(inner);
            unsafe {
                self.write(token, msg).ok().unwrap();
            }
            return Ok(());
        }

        if inner.is_disconnected {
            return Err(SendTimeoutError::Disconnected(msg));
        }

        Context::with(|cx| {
            // Prepare for blocking until a receiver wakes us up.
            let oper = Operation::hook(token);
            let packet = Packet::<T>::message_on_stack(msg);
            inner
                .senders
                .register_with_packet(oper, &packet as *const Packet<T> as usize, cx);
            drop(inner);

            // Block the current thread.
            let sel = cx.wait_until(deadline);

            match sel {
                Selected::Waiting => unreachable!(),
                Selected::Aborted => {
                    self.inner.lock().senders.unregister(oper).unwrap();
                    let msg = unsafe { packet.msg.get().replace(None).unwrap() };
                    Err(SendTimeoutError::Timeout(msg))
                }
                Selected::Disconnected => {
                    self.inner.lock().senders.unregister(oper).unwrap();
                    let msg = unsafe { packet.msg.get().replace(None).unwrap() };
                    Err(SendTimeoutError::Disconnected(msg))
                }
                Selected::Operation(_) => {
                    // Wait until the message is read, then drop the packet.
                    packet.wait_ready();
                    Ok(())
                }
            }
        })
    }

    /// Attempts to receive a message without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let token = &mut Token::default();
        let mut inner = self.inner.lock();

        // If there's a waiting sender, pair up with it.
        if let Some(operation) = inner.senders.wake_one() {
            token.zero = operation.packet;
            drop(inner);
            unsafe { self.read(token).map_err(|_| TryRecvError::Disconnected) }
        } else if inner.is_disconnected {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    /// Receives a message from the channel.
    pub fn recv(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let token = &mut Token::default();
        let mut inner = self.inner.lock();

        // If there's a waiting sender, pair up with it.
        if let Some(operation) = inner.senders.wake_one() {
            token.zero = operation.packet;
            drop(inner);
            unsafe {
                return self.read(token).map_err(|_| RecvTimeoutError::Disconnected);
            }
        }

        if inner.is_disconnected {
            return Err(RecvTimeoutError::Disconnected);
        }

        Context::with(|cx| {
            // Prepare for blocking until a sender wakes us up.
            let oper = Operation::hook(token);
            let packet = Packet::<T>::empty_on_stack();
            inner
                .receivers
                .register_with_packet(oper, &packet as *const Packet<T> as usize, cx);
            drop(inner);

            // Block the current thread.
            let sel = cx.wait_until(deadline);

            match sel {
                Selected::Waiting => unreachable!(),
                Selected::Aborted => {
                    self.inner.lock().receivers.unregister(oper).unwrap();
                    Err(RecvTimeoutError::Timeout)
                }
                Selected::Disconnected => {
                    self.inner.lock().receivers.unregister(oper).unwrap();
                    Err(RecvTimeoutError::Disconnected)
                }
                Selected::Operation(_) => {
                    // Wait until the message is provided, then read it.
                    packet.wait_ready();
                    unsafe { Ok(packet.msg.get().replace(None).unwrap()) }
                }
            }
        })
    }

    /// Disconnects the channel and wakes up all blocked receivers.
    pub fn disconnect(&self) {
        let mut inner = self.inner.lock();

        if !inner.is_disconnected {
            inner.is_disconnected = true;
            inner.senders.disconnect();
            inner.receivers.disconnect();
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

impl<'a, T> SelectHandle for Receiver<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_recv(token, false)
    }

    fn retry(&self, token: &mut Token) -> bool {
        self.0.start_recv(token, true)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, oper: Operation, cx: &Context) -> bool {
        let packet = Box::into_raw(Packet::<T>::empty_on_heap());

        let mut inner = self.0.inner.lock();
        inner
            .receivers
            .register_with_packet(oper, packet as usize, cx);
        !inner.senders.can_wake_one() && !inner.is_disconnected
    }

    fn unregister(&self, oper: Operation) {
        if let Some(operation) = self.0.inner.lock().receivers.unregister(oper) {
            unsafe {
                drop(Box::from_raw(operation.packet as *mut Packet<T>));
            }
        }
    }

    fn accept(&self, token: &mut Token, cx: &Context) -> bool {
        token.zero = cx.wait_packet();
        true
    }

    fn state(&self) -> usize {
        self.0.inner.lock().senders.register_count()
    }
}

impl<'a, T> SelectHandle for Sender<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_send(token, false)
    }

    fn retry(&self, token: &mut Token) -> bool {
        self.0.start_send(token, true)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, oper: Operation, cx: &Context) -> bool {
        let packet = Box::into_raw(Packet::<T>::empty_on_heap());

        let mut inner = self.0.inner.lock();
        inner
            .senders
            .register_with_packet(oper, packet as usize, cx);
        !inner.receivers.can_wake_one() && !inner.is_disconnected
    }

    fn unregister(&self, oper: Operation) {
        if let Some(operation) = self.0.inner.lock().senders.unregister(oper) {
            unsafe {
                drop(Box::from_raw(operation.packet as *mut Packet<T>));
            }
        }
    }

    fn accept(&self, token: &mut Token, cx: &Context) -> bool {
        token.zero = cx.wait_packet();
        true
    }

    fn state(&self) -> usize {
        self.0.inner.lock().receivers.register_count()
    }
}
