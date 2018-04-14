//! Channel implementation based on a linked list.
//!
//! This flavor has unbounded capacity.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};

use crossbeam_epoch::{self as epoch, Atomic, Guard, Owned};
use crossbeam_utils::cache_padded::CachePadded;

use monitor::Monitor;
use utils::Backoff;

#[derive(Copy, Clone)]
pub struct Token {
    pub entry: *const u8, // TODO: remove pub
    guard: usize,
}

/// Number of messages a node can hold.
const NODE_CAP: usize = 32;

/// An entry in a node of the linked list.
struct Entry<T> {
    /// The message in this entry.
    msg: ManuallyDrop<T>,

    /// Whether the message is ready for reading.
    ready: AtomicBool,
}

/// A node in the linked list.
///
/// Each node in the list can hold up to `NODE_CAP` messages. Storing multiple messages in a node
/// improves cache locality and reduces the total number of allocations.
struct Node<T> {
    /// The start index of this node.
    start_index: usize,

    /// The entries containing messages.
    entries: [UnsafeCell<Entry<T>>; NODE_CAP],

    /// The next node in the linked list.
    next: Atomic<Node<T>>,
}

impl<T> Node<T> {
    /// Returns a new, empty node that starts at `start_index`.
    fn new(start_index: usize) -> Node<T> {
        Node {
            start_index,
            entries: unsafe { mem::zeroed() },
            next: Atomic::null(),
        }
    }
}

/// A position in the channel (index and node).
///
/// This struct marks the current position of the head or the tail in a linked list.
struct Position<T> {
    /// The index in the channel.
    index: AtomicUsize,

    /// The node in the linked list.
    node: Atomic<Node<T>>,
}

/// A channel of unbounded capacity based on a linked list.
///
/// The internal queue can be thought of as an array of infinite length, implemented as a linked
/// list of nodes, each of which has enough space to contain a few dozen messages. Fitting multiple
/// messages into a single node improves cache locality and reduces the number of allocations.
///
/// An index is a number of type `usize` that represents an entry in the message queue. Each node
/// contains a `start_index` representing the index of its first message. Indices simply wrap
/// around on overflow. Also note that the last bit of an index is reserved for marking, while the
/// rest of the bits represent the actual position in the sequence of messages. When the tail index
/// is marked, that means the channel is closed and the tail cannot move forward any further.
pub struct Channel<T> {
    /// The current head index and the node containing it.
    head: CachePadded<Position<T>>,

    /// The current tail index and the node containing it.
    tail: CachePadded<Position<T>>,

    /// Receivers waiting on empty channel.
    receivers: Monitor,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        let channel = Channel {
            head: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                node: Atomic::null(),
            }),
            tail: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                node: Atomic::null(),
            }),
            receivers: Monitor::new(),
            _marker: PhantomData,
        };

        // Create an empty node, into which both head and tail point at the beginning.
        let node = unsafe { Owned::new(Node::new(0)).into_shared(epoch::unprotected()) };
        channel.head.node.store(node, Relaxed);
        channel.tail.node.store(node, Relaxed);

        channel
    }

    pub fn send(&self, msg: T, backoff: &mut Backoff) {
        let guard = epoch::pin();

        loop {
            // These two load operations don't have to be `SeqCst`. If they happen to retrieve
            // stale values, the following CAS will fail or not even be attempted.
            let tail_ptr = self.tail.node.load(Acquire, &guard);
            let tail = unsafe { tail_ptr.deref() };
            let tail_index = self.tail.index.load(Relaxed);

            // Calculate the index of the corresponding entry in the node.
            let offset = tail_index.wrapping_sub(tail.start_index) >> 1;

            // Advance the current index one entry forward.
            let new_index = tail_index.wrapping_add(1 << 1);

            // If `tail_index` is pointing into `tail`...
            if offset < NODE_CAP {
                // Try moving the tail index forward.
                if self.tail.index.compare_and_swap(tail_index, new_index, SeqCst) == tail_index {
                    // If this was the last entry in the node, allocate a new one.
                    if offset + 1 == NODE_CAP {
                        let new = Owned::new(Node::new(new_index)).into_shared(&guard);
                        tail.next.store(new, Release);
                        self.tail.node.store(new, Release);
                    }

                    // Write `msg` into the corresponding entry.
                    unsafe {
                        let entry = tail.entries.get_unchecked(offset).get();
                        ptr::write(&mut (*entry).msg, ManuallyDrop::new(msg));
                        (*entry).ready.store(true, Release);
                    }

                    if let Some(case) = self.receivers.remove_one() {
                        case.handle.unpark();
                    }
                    return;
                }
            }

            backoff.step();
        }
    }

    pub fn start_recv(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        let guard = epoch::pin();

        loop {
            // Loading the head node doesn't't have to be a `SeqCst` operation. If we get a stale
            // value, the following CAS will fail or not even be attempted. Loading the head index
            // must be `SeqCst` because we need the up-to-date value when checking whether the
            // channel is empty.
            let head_ptr = self.head.node.load(Acquire, &guard);
            let head = unsafe { head_ptr.deref() };
            let head_index = self.head.index.load(SeqCst);

            // Calculate the index of the corresponding entry in the node.
            let offset = head_index.wrapping_sub(head.start_index) >> 1;

            // Advance the current index one entry forward.
            let new_index = head_index.wrapping_add(1 << 1);

            // If `head_index` is pointing into `head`...
            if offset < NODE_CAP {
                let entry = unsafe { &*head.entries.get_unchecked(offset).get() };

                // If this entry does not contain a message...
                if !entry.ready.load(Relaxed) {
                    let tail_index = self.tail.index.load(SeqCst);

                    // If the tail equals the head, that means the channel is empty.
                    if tail_index & !1 == head_index {
                        // Check whether the channel is closed and return the appropriate
                        // error variant.
                        if tail_index & 1 == 0 {
                            return false;
                        } else {
                            token.entry = ptr::null();
                            return true;
                        }
                    }
                }

                // Try moving the head index forward.
                if self.head.index.compare_and_swap(head_index, new_index, SeqCst) == head_index {
                    // If this was the last entry in the node, defer its destruction.
                    if offset + 1 == NODE_CAP {
                        // Wait until the next pointer becomes non-null.
                        loop {
                            let next = head.next.load(Acquire, &guard);
                            if !next.is_null() {
                                self.head.node.store(next, Release);
                                break;
                            }
                            backoff.step();
                        }

                        unsafe {
                            guard.defer(move || head_ptr.into_owned());
                        }
                    }

                    token.entry = entry as *const Entry<T> as *const u8;
                    break;
                }
            }

            backoff.step();
        }

        token.guard = unsafe { mem::transmute(guard) };
        true
    }

    pub unsafe fn finish_recv(&self, token: Token) -> Option<T> {
        if token.entry.is_null() {
            None
        } else {
            let entry = &*(token.entry as *const Entry<T>);
            let _guard: Guard = mem::transmute(token.guard);

            let mut backoff = Backoff::new();
            while !entry.ready.load(Acquire) {
                backoff.step();
            }

            let m = ptr::read(&entry.msg);
            let msg = ManuallyDrop::into_inner(m);
            Some(msg)
        }
    }

    /// Returns the current number of messages inside the channel.
    pub fn len(&self) -> usize {
        loop {
            let tail_index = self.tail.index.load(SeqCst);
            let head_index = self.head.index.load(SeqCst);

            // If the tail index didn't change, we've got consistent indices to work with.
            if self.tail.index.load(SeqCst) == tail_index {
                // Note that there is no need to clear out the last bit in `tail_index` since the
                // difference is shifted right by one bit.
                return tail_index.wrapping_sub(head_index) >> 1;
            }
        }
    }

    /// Closes the channel and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        let tail_index = self.tail.index.fetch_or(1, SeqCst);

        // Was the channel already closed?
        if tail_index & 1 != 0 {
            false
        } else {
            self.receivers.abort_all();
            true
        }
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.tail.index.load(SeqCst) & 1 != 0
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        let head_index = self.head.index.load(SeqCst);
        let tail_index = self.tail.index.load(SeqCst) & !1;
        head_index == tail_index
    }

    /// Returns a reference to the monitor for this channel's receivers.
    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        let tail_index = self.tail.index.load(Relaxed) & !1;
        let mut head_index = self.head.index.load(Relaxed);

        unsafe {
            let mut head_ptr = self.head.node.load(Relaxed, epoch::unprotected());

            // Manually drop all messages between `head_index` and `tail_index` and destroy the
            // heap-allocated nodes along the way.
            while head_index != tail_index {
                let head = head_ptr.deref();
                let offset = head_index.wrapping_sub(head.start_index) >> 1;

                let entry = &mut *head.entries.get_unchecked(offset).get();
                ManuallyDrop::drop(&mut (*entry).msg);

                if offset + 1 == NODE_CAP {
                    let next = head.next.load(Relaxed, epoch::unprotected());
                    drop(head_ptr.into_owned());
                    head_ptr = next;
                }

                head_index = head_index.wrapping_add(1 << 1);
            }

            // If there is one last remaining node in the end, destroy it.
            if !head_ptr.is_null() {
                drop(head_ptr.into_owned());
            }
        }
    }
}
