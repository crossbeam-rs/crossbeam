//! Unbounded channel implemented as a linked list.

use std::alloc;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use crossbeam_epoch::{self as epoch, Atomic, Guard, Shared};
use crossbeam_utils::CachePadded;

use internal::channel::RecvNonblocking;
use internal::context::Context;
use internal::select::{Operation, SelectHandle, Selected, Token};
use internal::utils::Backoff;
use internal::waker::SyncWaker;

/// The maximum number of slots a block can hold.
const MAX_BLOCK_CAP: usize = 32;

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

/// A slot in a block.
struct Slot<T> {
    /// The message.
    msg: UnsafeCell<ManuallyDrop<T>>,

    /// Equals `true` if the message is ready for reading.
    ready: AtomicBool,
}

/// A block in a linked list.
///
/// Each block in the list can hold up to `cap` slots. `cap` will double for each consecutive block
/// - starting at 1 and growing no larger than `BLOCK_CAP`.
#[repr(C)]
struct Block<T> {
    /// The start index of this block.
    ///
    /// Slots in this block have indices in `start_index .. start_index + cap`.
    start_index: usize,

    /// The number of slots this block can hold.
    cap: usize,

    /// The next block in the linked list.
    next: Atomic<Block<T>>,

    /// Slots for messages.
    slots: [Slot<T>; 0],
}

impl<T> Block<T> {
    /// Creates an empty block that starts at `start_index`.
    fn new<'a>(start_index: usize, cap: usize) -> Shared<'a, Block<T>> {
        let block = unsafe {
            let ptr = Self::alloc(cap);

            // Write the `start_index` and `cap`. Initialize `next` to null and zero out `slots`.
            ptr::write(&mut (*ptr).start_index, start_index);
            ptr::write(&mut (*ptr).cap, cap);
            ptr::write(&mut (*ptr).next, Atomic::null());
            ptr::write_bytes((*ptr).slots.as_mut_ptr(), 0, cap);

            ptr
        };

        Shared::from(block as *const _)
    }

    /// Allocate memory for a block with space to hold `cap` slots.
    unsafe fn alloc(cap: usize) -> *mut Self {
        let layout = Self::create_layout(cap);
        // TODO(kleimkuhler): Use `GlobalAlloc::alloc` once `Global` becomes stable
        alloc::alloc(layout) as *mut Self
    }

    /// Deallocate memory that a block holds.
    unsafe fn dealloc(ptr: Shared<Block<T>>) {
        let ptr = ptr.as_raw() as *mut Block<T>;
        let layout = Self::create_layout((*ptr).cap);
        // TODO(kleimkuhler): Use `GlobalAlloc::dealloc` once `Global` becomes stable
        alloc::dealloc(ptr as *mut u8, layout)
    }

    /// Layout a block of memory for a block that can hold `cap` slots.
    unsafe fn create_layout(cap: usize) -> alloc::Layout {
        let size = mem::size_of::<Block<T>>() + cap * mem::size_of::<Slot<T>>();
        let align = mem::align_of::<Block<T>>();
        alloc::Layout::from_size_align_unchecked(size, align)
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

    /// Equals `true` when the channel is closed.
    is_closed: AtomicBool,

    /// Receivers waiting while the channel is empty and not closed.
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
            is_closed: AtomicBool::new(false),
            receivers: SyncWaker::new(),
            _marker: PhantomData,
        };

        // Allocate an empty block for the first batch of messages.
        let block = Block::new(0, 1);
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

    /// Writes a message into the channel.
    pub fn write(&self, _token: &mut Token, msg: T) {
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
                let new_slot_count = (tail.cap * 2).min(MAX_BLOCK_CAP);

                let current = tail
                    .next
                    .compare_and_set(
                        Shared::null(),
                        Block::new(tail.start_index.wrapping_add(tail.cap), new_slot_count),
                        Ordering::AcqRel,
                        &guard,
                    ).unwrap_or_else(|err| {
                        unsafe { Block::dealloc(err.new) };
                        err.current
                    });

                let _ =
                    self.tail
                        .block
                        .compare_and_set(tail_ptr, current, Ordering::Release, &guard);
            };

            // If `tail_index` is pointing into `tail`...
            if offset < tail.cap {
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
                    if offset + 1 == tail.cap {
                        install_next_block();
                    }

                    unsafe {
                        let slot = tail.slots.get_unchecked(offset);
                        (*slot).msg.get().write(ManuallyDrop::new(msg));
                        (*slot).ready.store(true, Ordering::Release);
                    }
                    break;
                }

                backoff.spin();
            } else if offset == tail.cap {
                // Help install the next block.
                install_next_block();
            }
        }

        self.receivers.wake_one();
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
                let new_slot_count = (head.cap * 2).min(MAX_BLOCK_CAP);

                let current = head
                    .next
                    .compare_and_set(
                        Shared::null(),
                        Block::new(head.start_index.wrapping_add(head.cap), new_slot_count),
                        Ordering::AcqRel,
                        &guard,
                    ).unwrap_or_else(|err| {
                        unsafe { Block::dealloc(err.new) };
                        err.current
                    });

                let _ =
                    self.head
                        .block
                        .compare_and_set(head_ptr, current, Ordering::Release, &guard);
            };

            // If `head_index` is pointing into `head`...
            if offset < head.cap {
                let slot = unsafe { &*head.slots.get_unchecked(offset) };

                // If this slot does not contain a message...
                if !slot.ready.load(Ordering::Relaxed) {
                    let tail_index = self.tail.index.load(Ordering::SeqCst);

                    // If the tail equals the head, that means the channel is empty.
                    if tail_index == head_index {
                        // If the channel is closed...
                        if self.is_closed() {
                            // ...and still empty...
                            if self.tail.index.load(Ordering::SeqCst) == tail_index {
                                // ...then receive `None`.
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
                    if offset + 1 == head.cap {
                        install_next_block();
                        unsafe {
                            guard.defer_unchecked(move || Block::dealloc(head_ptr));
                        }
                    }

                    token.list.slot = slot as *const Slot<T> as *const u8;
                    break;
                }

                backoff.spin();
            } else if offset == head.cap {
                // Help install the next block.
                install_next_block();
            }
        }

        token.list.guard = Some(guard);
        true
    }

    /// Reads a message from the channel.
    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        if token.list.slot.is_null() {
            // The channel is closed.
            return None;
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
        Some(msg)
    }

    /// Sends a message into the channel.
    pub fn send(&self, msg: T) {
        let token = &mut Token::default();
        self.write(token, msg);
    }

    /// Receives a message from the channel.
    pub fn recv(&self) -> Option<T> {
        let token = &mut Token::default();
        loop {
            // Try receiving a message several times.
            let mut backoff = Backoff::new();
            loop {
                if self.start_recv(token) {
                    unsafe {
                        return self.read(token);
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
                if !self.is_empty() || self.is_closed() {
                    let _ = cx.try_select(Selected::Aborted);
                }

                // Block the current thread.
                let sel = cx.wait_until(None);

                match sel {
                    Selected::Waiting => unreachable!(),
                    Selected::Aborted | Selected::Closed => {
                        self.receivers.unregister(oper).unwrap();
                        // If the channel was closed, we still have to check for remaining messages.
                    }
                    Selected::Operation(_) => {}
                }
            })
        }
    }

    /// Attempts to receive a message without blocking.
    pub fn recv_nonblocking(&self) -> RecvNonblocking<T> {
        let token = &mut Token::default();

        if self.start_recv(token) {
            match unsafe { self.read(token) } {
                None => RecvNonblocking::Closed,
                Some(msg) => RecvNonblocking::Message(msg),
            }
        } else {
            RecvNonblocking::Empty
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

    /// Closes the channel and wakes up all blocked receivers.
    pub fn close(&self) {
        assert!(!self.is_closed.swap(true, Ordering::SeqCst));
        self.receivers.close();
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::SeqCst)
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

                let slot = head.slots.get_unchecked(offset);
                ManuallyDrop::drop(&mut *(*slot).msg.get());

                if offset + 1 == head.cap {
                    let next = head.next.load(Ordering::Relaxed, epoch::unprotected());
                    Block::dealloc(head_ptr);
                    head_ptr = next;
                }

                head_index = head_index.wrapping_add(1);
            }

            // If there is one last remaining block in the end, destroy it.
            if !head_ptr.is_null() {
                Block::dealloc(head_ptr);
            }
        }
    }
}

/// Receiver handle to a channel.
pub struct Receiver<'a, T: 'a>(&'a Channel<T>);

/// Sender handle to a channel.
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> SelectHandle for Receiver<'a, T> {
    fn try(&self, token: &mut Token) -> bool {
        self.0.start_recv(token)
    }

    fn retry(&self, token: &mut Token) -> bool {
        self.0.start_recv(token)
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, oper: Operation, cx: &Context) -> bool {
        self.0.receivers.register(oper, cx);
        self.0.is_empty() && !self.0.is_closed()
    }

    fn unregister(&self, oper: Operation) {
        self.0.receivers.unregister(oper);
    }

    fn accept(&self, token: &mut Token, _cx: &Context) -> bool {
        self.0.start_recv(token)
    }

    fn state(&self) -> usize {
        self.0.tail.index.load(Ordering::SeqCst)
    }
}

impl<'a, T> SelectHandle for Sender<'a, T> {
    fn try(&self, _token: &mut Token) -> bool {
        true
    }

    fn retry(&self, _token: &mut Token) -> bool {
        true
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, _token: &mut Token, _oper: Operation, _cx: &Context) -> bool {
        false
    }

    fn unregister(&self, _oper: Operation) {}

    fn accept(&self, _token: &mut Token, _cx: &Context) -> bool {
        true
    }

    fn state(&self) -> usize {
        self.0.head.index.load(Ordering::SeqCst)
    }
}
