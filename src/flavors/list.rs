//! Channel implementation based on a linked list.
//!
//! This flavor has unbounded capacity.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::time::Instant;

use crossbeam_epoch::{self as epoch, Atomic, Owned};
use crossbeam_utils::cache_padded::CachePadded;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use monitor::Monitor;
use select::CaseId;
use select::handle;
use utils::Backoff;

/// Number of values a node can hold.
const NODE_CAP: usize = 32;

/// An entry in a node of the linked list.
struct Entry<T> {
    /// The value in this entry.
    value: ManuallyDrop<T>,

    /// Whether the value is ready for reading.
    ready: AtomicBool,
}

/// A node in the linked list.
///
/// Each node in the list can hold up to `NODE_CAP` values. Storing multiple values in a node
/// improves cache locality and reduces the total amount of allocation.
struct Node<T> {
    /// Start index of this node.
    /// Indices span the range `start_index .. start_index + NODE_CAP`.
    start_index: usize,

    /// Entries containing values.
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

/// A position in the queue (index and node).
///
/// This struct marks the current position of the head or the tail in a linked list.
struct Position<T> {
    /// The index in the queue.
    ///
    /// Indices wrap around on overflow.
    index: AtomicUsize,

    /// The node in the linked list.
    node: Atomic<Node<T>>,
}

/// A channel of unbounded capacity based on a linked list.
///
/// The internal queue can be thought of as an array of infinite length. Head and tail indices
/// point into the array and wrap around on overflow. This infinite array is implemented as a
/// linked list of nodes, each of which has enough space to contain a few dozen values.
pub struct Channel<T> {
    /// The current head index and the node containing it.
    head: CachePadded<Position<T>>,

    /// The current tail index and the node containing it.
    tail: CachePadded<Position<T>>,

    /// Equals `true` if the channel is disconnected.
    is_disconnected: AtomicBool,

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
            is_disconnected: AtomicBool::new(false),
            receivers: Monitor::new(),
            _marker: PhantomData,
        };

        // Create an empty node, into which both head and tail point at the beginning.
        let node = unsafe { Owned::new(Node::new(0)).into_shared(epoch::unprotected()) };
        channel.head.node.store(node, Relaxed);
        channel.tail.node.store(node, Relaxed);

        channel
    }

    /// Pushes `value` into the queue.
    fn push(&self, value: T) {
        let guard = &epoch::pin();

        let mut backoff = Backoff::new();
        loop {
            let tail = unsafe { self.tail.node.load(Acquire, guard).deref() };
            let index = self.tail.index.load(Relaxed);
            let new_index = index.wrapping_add(1);
            let offset = index.wrapping_sub(tail.start_index);

            // If `index` is pointing into `tail`...
            if offset < NODE_CAP {
                // if `index` is pointing into `tail`, try moving the tail index forward.
                if self.tail.index.compare_and_swap(index, new_index, SeqCst) == index {
                    // If this was the last entry in the node, allocate a new one.
                    if offset + 1 == NODE_CAP {
                        let new = Owned::new(Node::new(new_index)).into_shared(guard);
                        tail.next.store(new, Release);
                        self.tail.node.store(new, Release);
                    }

                    // Write `value` into the corresponding entry.
                    unsafe {
                        let entry = tail.entries.get_unchecked(offset).get();
                        ptr::write(&mut (*entry).value, ManuallyDrop::new(value));
                        (*entry).ready.store(true, Release);
                    }
                    return;
                }
            }

            backoff.step();
        }
    }

    /// Attempts to pop a value from the queue.
    ///
    /// Returns `None` if the queue is empty.
    fn pop(&self) -> Option<T> {
        let guard = &epoch::pin();

        let mut backoff = Backoff::new();
        loop {
            let head_ptr = self.head.node.load(Acquire, guard);
            let head = unsafe { head_ptr.deref() };
            let index = self.head.index.load(SeqCst);
            let new_index = index.wrapping_add(1);
            let offset = index.wrapping_sub(head.start_index);

            // If `index` is pointing into `head`...
            if offset < NODE_CAP {
                let entry = unsafe { &*head.entries.get_unchecked(offset).get() };

                // If this entry does not contain the value and the tail equals the head, then the
                // queue is empty.
                if !entry.ready.load(Relaxed) && self.tail.index.load(SeqCst) == index {
                    return None;
                }

                // Try moving the head index forward.
                if self.head.index.compare_and_swap(index, new_index, SeqCst) == index {
                    // If this was the last entry in the node, defer its destruction.
                    if offset + 1 == NODE_CAP {
                        // Wait until the next pointer becomes non-null.
                        loop {
                            let next = head.next.load(Acquire, guard);
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

                    while !entry.ready.load(Acquire) {
                        backoff.step();
                    }

                    let v = unsafe { ptr::read(&(*entry).value) };
                    let value = ManuallyDrop::into_inner(v);
                    return Some(value);
                }
            }

            backoff.step();
        }
    }

    /// Returns the current number of messages inside the channel.
    pub fn len(&self) -> usize {
        loop {
            let tail_index = self.tail.index.load(SeqCst);
            let head_index = self.head.index.load(SeqCst);

            // If the tail index didn't change, we've got consistent indices to work with.
            if self.tail.index.load(SeqCst) == tail_index {
                return tail_index.wrapping_sub(head_index);
            }
        }
    }

    /// Attempts to send `msg` into the channel.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        if self.is_disconnected.load(SeqCst) {
            Err(TrySendError::Disconnected(msg))
        } else {
            self.push(msg);
            self.receivers.notify_one();
            Ok(())
        }
    }

    /// Send `msg` into the channel.
    pub fn send(&self, msg: T) -> Result<(), SendTimeoutError<T>> {
        if self.is_disconnected.load(SeqCst) {
            Err(SendTimeoutError::Disconnected(msg))
        } else {
            self.push(msg);
            self.receivers.notify_one();
            Ok(())
        }
    }

    /// Attempts to receive a message from channel.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let is_disconnected = self.is_disconnected.load(SeqCst);
        match self.pop() {
            None => {
                if is_disconnected {
                    Err(TryRecvError::Disconnected)
                } else {
                    Err(TryRecvError::Empty)
                }
            }
            Some(msg) => Ok(msg),
        }
    }

    /// Attempts to receive a message from the channel until the specified `deadline`.
    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        loop {
            let backoff = &mut Backoff::new();
            loop {
                let is_disconnected = self.is_disconnected.load(SeqCst);
                if let Some(msg) = self.pop() {
                    return Ok(msg);
                }
                if is_disconnected {
                    return Err(RecvTimeoutError::Disconnected);
                }
                if !backoff.step() {
                    break;
                }
            }

            handle::current_reset();
            self.receivers.register(case_id);
            let is_disconnected = self.is_disconnected();
            let timed_out =
                !is_disconnected && self.is_empty() && !handle::current_wait_until(deadline);
            self.receivers.unregister(case_id);

            if is_disconnected && self.is_empty() {
                return Err(RecvTimeoutError::Disconnected);
            } else if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    /// Disconnects the channel and wakes up all currently blocked operations on it.
    pub fn disconnect(&self) -> bool {
        if self.is_disconnected.swap(true, SeqCst) {
            false
        } else {
            self.receivers.abort_all();
            true
        }
    }

    /// Returns `true` if the channel is disconnected.
    pub fn is_disconnected(&self) -> bool {
        self.is_disconnected.load(SeqCst)
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        let head_index = self.head.index.load(SeqCst);
        let tail_index = self.tail.index.load(SeqCst);
        head_index == tail_index
    }

    /// Returns a reference to the monitor for this channel's receivers.
    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        let tail_index = self.tail.index.load(Relaxed);
        let mut head_index = self.head.index.load(Relaxed);

        unsafe {
            let mut head_ptr = self.head.node.load(Relaxed, epoch::unprotected());

            // Manually drop all values between `head_index` and `tail_index` and destroy the
            // heap-allocated nodes along the way.
            while head_index != tail_index {
                let head = head_ptr.deref();
                let offset = head_index.wrapping_sub(head.start_index);

                let entry = &mut *head.entries.get_unchecked(offset).get();
                ManuallyDrop::drop(&mut (*entry).value);

                if offset + 1 == NODE_CAP {
                    let next = head.next.load(Relaxed, epoch::unprotected());
                    drop(head_ptr.into_owned());
                    head_ptr = next;
                }

                head_index = head_index.wrapping_add(1);
            }

            // If there is one last remaining node in the end, destroy it.
            if !head_ptr.is_null() {
                drop(head_ptr.into_owned());
            }
        }
    }
}
