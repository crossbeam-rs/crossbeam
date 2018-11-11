//! Unbounded channel implemented as a linked list.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use crossbeam_epoch::{self as epoch, Atomic, Guard, Owned, Shared};
use crossbeam_utils::CachePadded;

use context::Context;
use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use select::{Operation, SelectHandle, Selected, Token};
use utils::Backoff;
use waker::SyncWaker;

// TODO(stjepang): Once we bump the minimum required Rust version to 1.28 or newer, reapply the
// following changes by @kleimkuhler:
//
// 1. https://github.com/crossbeam-rs/crossbeam-channel/pull/100
// 2. https://github.com/crossbeam-rs/crossbeam-channel/pull/101

/// The maximum number of messages a block can hold.
const BLOCK_CAP: usize = 32;

/// A slot in a block.
struct Slot<T> {
    /// The message.
    msg: UnsafeCell<ManuallyDrop<T>>,

    /// Equals `true` if the message is ready for reading.
    ready: AtomicBool,
}

/// The token type for the list flavor.
pub struct ListToken {
    /// Slot to read from or write to.
    slot: *const u8,

    /// Guard keeping alive the block that contains the slot.
    guard: Option<Guard>,
}

impl Default for ListToken {
    #[inline]
    fn default() -> Self {
        ListToken {
            slot: ptr::null(),
            guard: None,
        }
    }
}

/// A block in a linked list.
///
/// Each block in the list can hold up to `BLOCK_CAP` messages.
struct Block<T> {
    /// The start index of this block.
    ///
    /// Slots in this block have indices in `start_index .. start_index + BLOCK_CAP`.
    start_index: usize,

    /// The next block in the linked list.
    next: Atomic<Block<T>>,

    /// Slots for messages.
    slots: [UnsafeCell<Slot<T>>; BLOCK_CAP],
}

impl<T> Block<T> {
    /// Creates an empty block that starts at `start_index`.
    fn new(start_index: usize) -> Block<T> {
        Block {
            start_index,
            slots: unsafe { mem::zeroed() },
            next: Atomic::null(),
        }
    }
}

/// Position in the channel (index and block).
///
/// This struct describes the current position of the head or the tail in a linked list.
struct Position<T> {
    /// The index in the channel.
    index: AtomicUsize,

    /// The block in the linked list.
    block: Atomic<Block<T>>,
}

/// Unbounded channel implemented as a linked list.
///
/// Each message sent into the channel is assigned a sequence number, i.e. an index. Indices are
/// represented as numbers of type `usize` and wrap on overflow.
///
/// Consecutive messages are grouped into blocks in order to put less pressure on the allocator and
/// improve cache efficiency.
pub struct Channel<T> {
    /// The head of the channel.
    head: CachePadded<Position<T>>,

    /// The tail of the channel.
    tail: CachePadded<Position<T>>,

    /// Equals `true` when the channel is disconnected.
    is_disconnected: AtomicBool,

    /// Receivers waiting while the channel is empty and not disconnected.
    receivers: SyncWaker,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Creates a new unbounded channel.
    pub fn new() -> Self {
        let channel = Channel {
            head: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                block: Atomic::null(),
            }),
            tail: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                block: Atomic::null(),
            }),
            is_disconnected: AtomicBool::new(false),
            receivers: SyncWaker::new(),
            _marker: PhantomData,
        };

        // Allocate an empty block for the first batch of messages.
        let block = unsafe { Owned::new(Block::new(0)).into_shared(epoch::unprotected()) };
        channel.head.block.store(block, Ordering::Relaxed);
        channel.tail.block.store(block, Ordering::Relaxed);

        channel
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
        // If the channel is disconnected, return early.
        if self.is_disconnected() {
            token.list.slot = ptr::null();
            return true;
        }

        let guard = epoch::pin();
        let mut backoff = Backoff::new();

        loop {
            // These two load operations don't have to be `SeqCst`. If they happen to retrieve
            // stale values, the following CAS will fail or won't even be attempted.
            let tail_ptr = self.tail.block.load(Ordering::Acquire, &guard);
            let tail = unsafe { tail_ptr.deref() };
            let tail_index = self.tail.index.load(Ordering::Relaxed);

            // Calculate the index of the corresponding slot in the block.
            let offset = tail_index.wrapping_sub(tail.start_index);

            // Advance the current index one slot forward.
            let new_index = tail_index.wrapping_add(1);

            // A closure that installs a block following `tail` in case it hasn't been yet.
            let install_next_block = || {
                let current = tail
                    .next
                    .compare_and_set(
                        Shared::null(),
                        Owned::new(Block::new(tail.start_index.wrapping_add(BLOCK_CAP))),
                        Ordering::AcqRel,
                        &guard,
                    ).unwrap_or_else(|err| err.current);

                let _ =
                    self.tail
                        .block
                        .compare_and_set(tail_ptr, current, Ordering::Release, &guard);
            };

            // If `tail_index` is pointing into `tail`...
            if offset < BLOCK_CAP {
                // Try moving the tail index forward.
                if self
                    .tail
                    .index
                    .compare_exchange_weak(
                        tail_index,
                        new_index,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_ok()
                {
                    // If this was the last slot in the block, install a new block.
                    if offset + 1 == BLOCK_CAP {
                        install_next_block();
                    }

                    unsafe {
                        let slot = tail.slots.get_unchecked(offset).get();
                        token.list.slot = slot as *const Slot<T> as *const u8;
                    }
                    break;
                }

                backoff.spin();
            } else if offset == BLOCK_CAP {
                // Help install the next block.
                install_next_block();
            }
        }

        token.list.guard = Some(guard);
        true
    }

    /// Writes a message into the channel.
    pub unsafe fn write(&self, token: &mut Token, msg: T) -> Result<(), T> {
        // If there is no slot, the channel is disconnected.
        if token.list.slot.is_null() {
            return Err(msg);
        }

        let slot = &*(token.list.slot as *const Slot<T>);
        let _guard: Guard = token.list.guard.take().unwrap();

        // Write the message into the slot.
        (*slot).msg.get().write(ManuallyDrop::new(msg));
        (*slot).ready.store(true, Ordering::Release);

        // Wake a sleeping receiver.
        self.receivers.notify();
        Ok(())
    }

    /// Attempts to reserve a slot for receiving a message.
    fn start_recv(&self, token: &mut Token) -> bool {
        let guard = epoch::pin();
        let mut backoff = Backoff::new();

        loop {
            // Loading the head block doesn't have to be a `SeqCst` operation. If we get a stale
            // value, the following CAS will fail or not even be attempted. Loading the head index
            // must be `SeqCst` because we need the up-to-date value when checking whether the
            // channel is empty.
            let head_ptr = self.head.block.load(Ordering::Acquire, &guard);
            let head = unsafe { head_ptr.deref() };
            let head_index = self.head.index.load(Ordering::SeqCst);

            // Calculate the index of the corresponding slot in the block.
            let offset = head_index.wrapping_sub(head.start_index);

            // Advance the current index one slot forward.
            let new_index = head_index.wrapping_add(1);

            // A closure that installs a block following `head` in case it hasn't been yet.
            let install_next_block = || {
                let current = head
                    .next
                    .compare_and_set(
                        Shared::null(),
                        Owned::new(Block::new(head.start_index.wrapping_add(BLOCK_CAP))),
                        Ordering::AcqRel,
                        &guard,
                    ).unwrap_or_else(|err| err.current);

                let _ =
                    self.head
                        .block
                        .compare_and_set(head_ptr, current, Ordering::Release, &guard);
            };

            // If `head_index` is pointing into `head`...
            if offset < BLOCK_CAP {
                let slot = unsafe { &*head.slots.get_unchecked(offset).get() };

                // If this slot does not contain a message...
                if !slot.ready.load(Ordering::Relaxed) {
                    let tail_index = self.tail.index.load(Ordering::SeqCst);

                    // If the tail equals the head, that means the channel is empty.
                    if tail_index == head_index {
                        // If the channel is disconnected...
                        if self.is_disconnected() {
                            // ...and still empty...
                            if self.tail.index.load(Ordering::SeqCst) == tail_index {
                                // ...then receive an error.
                                token.list.slot = ptr::null();
                                return true;
                            }
                        } else {
                            // Otherwise, the receive operation is not ready.
                            return false;
                        }
                    }
                }

                // Try moving the head index forward.
                if self
                    .head
                    .index
                    .compare_exchange_weak(
                        head_index,
                        new_index,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_ok()
                {
                    // If this was the last slot in the block, install a new block and destroy the
                    // old one.
                    if offset + 1 == BLOCK_CAP {
                        install_next_block();
                        unsafe {
                            guard.defer_destroy(head_ptr);
                        }
                    }

                    token.list.slot = slot as *const Slot<T> as *const u8;
                    break;
                }

                backoff.spin();
            } else if offset == BLOCK_CAP {
                // Help install the next block.
                install_next_block();
            }
        }

        token.list.guard = Some(guard);
        true
    }

    /// Reads a message from the channel.
    pub unsafe fn read(&self, token: &mut Token) -> Result<T, ()> {
        if token.list.slot.is_null() {
            // The channel is disconnected.
            return Err(());
        }

        let slot = &*(token.list.slot as *const Slot<T>);
        let _guard: Guard = token.list.guard.take().unwrap();

        // Wait until the message becomes ready.
        let mut backoff = Backoff::new();
        while !slot.ready.load(Ordering::Acquire) {
            backoff.snooze();
        }

        // Read the message.
        let m = slot.msg.get().read();
        let msg = ManuallyDrop::into_inner(m);
        Ok(msg)
    }

    /// Attempts to send a message into the channel.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        self.send(msg, None).map_err(|err| match err {
            SendTimeoutError::Disconnected(msg) => TrySendError::Disconnected(msg),
            SendTimeoutError::Timeout(_) => unreachable!(),
        })
    }

    /// Sends a message into the channel.
    pub fn send(&self, msg: T, _deadline: Option<Instant>) -> Result<(), SendTimeoutError<T>> {
        let token = &mut Token::default();
        assert!(self.start_send(token));
        unsafe {
            self.write(token, msg)
                .map_err(SendTimeoutError::Disconnected)
        }
    }

    /// Attempts to receive a message without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let token = &mut Token::default();

        if self.start_recv(token) {
            unsafe { self.read(token).map_err(|_| TryRecvError::Disconnected) }
        } else {
            Err(TryRecvError::Empty)
        }
    }

    /// Receives a message from the channel.
    pub fn recv(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let token = &mut Token::default();
        loop {
            // Try receiving a message several times.
            let mut backoff = Backoff::new();
            loop {
                if self.start_recv(token) {
                    unsafe {
                        return self.read(token).map_err(|_| RecvTimeoutError::Disconnected);
                    }
                }
                if !backoff.snooze() {
                    break;
                }
            }

            // Prepare for blocking until a sender wakes us up.
            Context::with(|cx| {
                let oper = Operation::hook(token);
                self.receivers.register(oper, cx);

                // Has the channel become ready just now?
                if !self.is_empty() || self.is_disconnected() {
                    let _ = cx.try_select(Selected::Aborted);
                }

                // Block the current thread.
                let sel = cx.wait_until(deadline);

                match sel {
                    Selected::Waiting => unreachable!(),
                    Selected::Aborted | Selected::Disconnected => {
                        self.receivers.unregister(oper).unwrap();
                        // If the channel was disconnected, we still have to check for remaining
                        // messages.
                    }
                    Selected::Operation(_) => {}
                }
            });

            if let Some(d) = deadline {
                if Instant::now() >= d {
                    return Err(RecvTimeoutError::Timeout);
                }
            }
        }
    }

    /// Returns the current number of messages inside the channel.
    pub fn len(&self) -> usize {
        loop {
            // Load the tail index, then load the head index.
            let tail_index = self.tail.index.load(Ordering::SeqCst);
            let head_index = self.head.index.load(Ordering::SeqCst);

            // If the tail index didn't change, we've got consistent indices to work with.
            if self.tail.index.load(Ordering::SeqCst) == tail_index {
                return tail_index.wrapping_sub(head_index);
            }
        }
    }

    /// Returns the capacity of the channel.
    pub fn capacity(&self) -> Option<usize> {
        None
    }

    /// Disconnects the channel and wakes up all blocked receivers.
    pub fn disconnect(&self) {
        if !self.is_disconnected.swap(true, Ordering::SeqCst) {
            self.receivers.disconnect();
        }
    }

    /// Returns `true` if the channel is disconnected.
    pub fn is_disconnected(&self) -> bool {
        self.is_disconnected.load(Ordering::SeqCst)
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        let head_index = self.head.index.load(Ordering::SeqCst);
        let tail_index = self.tail.index.load(Ordering::SeqCst);
        head_index == tail_index
    }

    /// Returns `true` if the channel is full.
    pub fn is_full(&self) -> bool {
        false
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        // Get the tail and head indices.
        let tail_index = self.tail.index.load(Ordering::Relaxed);
        let mut head_index = self.head.index.load(Ordering::Relaxed);

        unsafe {
            let mut head_ptr = self
                .head
                .block
                .load(Ordering::Relaxed, epoch::unprotected());

            // Manually drop all messages between `head_index` and `tail_index` and destroy the
            // heap-allocated nodes along the way.
            while head_index != tail_index {
                let head = head_ptr.deref();
                let offset = head_index.wrapping_sub(head.start_index);

                let slot = &mut *head.slots.get_unchecked(offset).get();
                ManuallyDrop::drop(&mut (*slot).msg.get().read());

                if offset + 1 == BLOCK_CAP {
                    let next = head.next.load(Ordering::Relaxed, epoch::unprotected());
                    drop(head_ptr.into_owned());
                    head_ptr = next;
                }

                head_index = head_index.wrapping_add(1);
            }

            // If there is one last remaining block in the end, destroy it.
            if !head_ptr.is_null() {
                drop(head_ptr.into_owned());
            }
        }
    }
}

/// Receiver handle to a channel.
pub struct Receiver<'a, T: 'a>(&'a Channel<T>);

/// Sender handle to a channel.
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> SelectHandle for Receiver<'a, T> {
    fn try_select(&self, token: &mut Token) -> bool {
        self.0.start_recv(token)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, oper: Operation, cx: &Context) -> bool {
        self.0.receivers.register(oper, cx);
        self.is_ready()
    }

    fn unregister(&self, oper: Operation) {
        self.0.receivers.unregister(oper);
    }

    fn accept(&self, token: &mut Token, _cx: &Context) -> bool {
        self.try_select(token)
    }

    fn is_ready(&self) -> bool {
        !self.0.is_empty() || self.0.is_disconnected()
    }

    fn watch(&self, oper: Operation, cx: &Context) -> bool {
        self.0.receivers.watch(oper, cx);
        self.is_ready()
    }

    fn unwatch(&self, oper: Operation) {
        self.0.receivers.unwatch(oper);
    }

    fn state(&self) -> usize {
        self.0.tail.index.load(Ordering::SeqCst)
    }
}

impl<'a, T> SelectHandle for Sender<'a, T> {
    fn try_select(&self, token: &mut Token) -> bool {
        self.0.start_send(token)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _oper: Operation, _cx: &Context) -> bool {
        self.is_ready()
    }

    fn unregister(&self, _oper: Operation) {}

    fn accept(&self, token: &mut Token, _cx: &Context) -> bool {
        self.try_select(token)
    }

    fn is_ready(&self) -> bool {
        true
    }

    fn watch(&self, _oper: Operation, _cx: &Context) -> bool {
        self.is_ready()
    }

    fn unwatch(&self, _oper: Operation) {}

    fn state(&self) -> usize {
        self.0.head.index.load(Ordering::SeqCst)
    }
}
