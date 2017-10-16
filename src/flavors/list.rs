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

use coco::epoch::{self, Atomic, Owned};
use crossbeam_utils::cache_padded::CachePadded;

use CaseId;
use actor;
use backoff::Backoff;
use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use monitor::Monitor;

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

    /// Equals `true` if the queue is closed.
    closed: AtomicBool,

    /// Receivers waiting on empty queue.
    receivers: Monitor,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        let queue = Channel {
            head: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                node: Atomic::null(),
            }),
            tail: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                node: Atomic::null(),
            }),
            closed: AtomicBool::new(false),
            receivers: Monitor::new(),
            _marker: PhantomData,
        };

        unsafe {
            // Create an empty node, into which both head and tail point at the beginning.
            epoch::unprotected(|scope| {
                let node = Owned::new(Node::new(0)).into_ptr(scope);
                queue.head.node.store(node, Relaxed);
                queue.tail.node.store(node, Relaxed);
            })
        }

        queue
    }

    /// Pushes `value` into the queue.
    fn push(&self, value: T) {
        epoch::pin(|scope| {
            let mut backoff = Backoff::new();
            loop {
                let tail = unsafe { self.tail.node.load(SeqCst, scope).deref() };
                let index = self.tail.index.load(SeqCst);
                let new_index = index.wrapping_add(1);
                let offset = index.wrapping_sub(tail.start_index);

                // If `index` is not pointing into `tail`, try again.
                if offset >= NODE_CAP {
                    backoff.step();
                    continue;
                }

                // Try moving the tail index forward.
                if self.tail.index.compare_and_swap(index, new_index, SeqCst) == index {
                    // If this was the last entry in the node, allocate a new one.
                    if offset + 1 == NODE_CAP {
                        let new = Owned::new(Node::new(new_index)).into_ptr(scope);
                        tail.next.store(new, SeqCst);
                        self.tail.node.store(new, SeqCst);
                    }

                    // Write `value` into the corresponding entry.
                    unsafe {
                        let entry = tail.entries.get_unchecked(offset).get();
                        ptr::write(&mut (*entry).value, ManuallyDrop::new(value));
                        (*entry).ready.store(true, SeqCst);
                    }
                    return;
                }
            }
        })
    }

    /// Attempts to pop a value from the queue.
    ///
    /// Returns `None` if the queue is empty.
    fn pop(&self) -> Option<T> {
        epoch::pin(|scope| {
            let mut backoff = Backoff::new();
            loop {
                let head_ptr = self.head.node.load(SeqCst, scope);
                let head = unsafe { head_ptr.deref() };
                let index = self.head.index.load(SeqCst);
                let new_index = index.wrapping_add(1);
                let offset = index.wrapping_sub(head.start_index);

                // If `index` is not pointing into `head`, try again.
                if offset >= NODE_CAP {
                    backoff.step();
                    continue;
                }

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
                            let next = head.next.load(SeqCst, scope);
                            if !next.is_null() {
                                self.head.node.store(next, SeqCst);
                                break;
                            }
                            backoff.step();
                        }

                        unsafe {
                            scope.defer_free(head_ptr);
                        }
                    }

                    while !entry.ready.load(SeqCst) {
                        backoff.step();
                    }

                    let v = unsafe { ptr::read(&(*entry).value) };
                    let value = ManuallyDrop::into_inner(v);
                    return Some(value);
                }
            }
        })
    }

    /// Returns the current number of values inside the channel.
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

    /// Attempts to send `value` into the channel.
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one();
            Ok(())
        }
    }

    /// Send `value` into the channel.
    pub fn send(&self, value: T) -> Result<(), SendTimeoutError<T>> {
        if self.closed.load(SeqCst) {
            Err(SendTimeoutError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one();
            Ok(())
        }
    }

    /// Attempts to receive a value from channel.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let closed = self.closed.load(SeqCst);
        match self.pop() {
            None => {
                if closed {
                    Err(TryRecvError::Disconnected)
                } else {
                    Err(TryRecvError::Empty)
                }
            },
            Some(v) => Ok(v),
        }
    }

    /// Attempts to receive a value from channel, retrying several times if it is empty.
    pub fn spin_try_recv(&self) -> Result<T, TryRecvError> {
        for _ in 0..20 {
            let closed = self.closed.load(SeqCst);
            if let Some(v) = self.pop() {
                return Ok(v);
            }
            if closed {
                return Err(TryRecvError::Disconnected);
            }
        }
        Err(TryRecvError::Empty)
    }

    /// Attempts to receive a value from the channel until the specified `deadline`.
    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        loop {
            match self.spin_try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            }

            actor::current_reset();
            self.receivers.register(case_id);
            let is_closed = self.is_closed();
            let timed_out = !is_closed && self.is_empty() && !actor::current_wait_until(deadline);
            self.receivers.unregister(case_id);

            if is_closed && self.is_empty() {
                return Err(RecvTimeoutError::Disconnected);
            } else if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    /// Closes the channel.
    pub fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            false
        } else {
            self.receivers.abort_all();
            true
        }
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        let tail_index = self.tail.index.load(Relaxed);
        let mut head_index = self.head.index.load(Relaxed);

        unsafe {
            epoch::unprotected(|scope| {
                let mut head_ptr = self.head.node.load(Relaxed, scope);

                // Manually drop all values between `head_index` and `tail_index` and destroy the
                // heap-allocated nodes along the way.
                while head_index != tail_index {
                    let head = head_ptr.deref();
                    let offset = head_index.wrapping_sub(head.start_index);

                    let entry = &mut *head.entries.get_unchecked(offset).get();
                    ManuallyDrop::drop(&mut (*entry).value);

                    if offset + 1 == NODE_CAP {
                        let next = head.next.load(Relaxed, scope);
                        head_ptr.destroy();
                        head_ptr = next;
                    }

                    head_index = head_index.wrapping_add(1);
                }

                // If there is one last remaining node in the end, destroy it.
                if !head_ptr.is_null() {
                    head_ptr.destroy();
                }
            })
        }
    }
}
