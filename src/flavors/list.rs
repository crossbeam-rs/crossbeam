//! Linked list-based channel implementation.
//!
//! This flavor is unbounded.

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

const NODE_LEN: usize = 32;

struct Entry<T> {
    value: ManuallyDrop<T>,
    ready: AtomicBool,
}

struct Node<T> {
    start: usize,
    entries: [UnsafeCell<Entry<T>>; NODE_LEN],
    next: Atomic<Node<T>>,
}

impl<T> Node<T> {
    fn new(start: usize) -> Node<T> {
        Node {
            start,
            entries: unsafe { mem::zeroed() },
            next: Atomic::null(),
        }
    }
}

struct Position<T> {
    index: AtomicUsize,
    node: Atomic<Node<T>>,
}

pub struct Channel<T> {
    head: CachePadded<Position<T>>,
    tail: CachePadded<Position<T>>,
    closed: AtomicBool,
    receivers: Monitor,
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
            epoch::unprotected(|scope| {
                let node = Owned::new(Node::new(0)).into_ptr(scope);
                queue.head.node.store(node, Relaxed);
                queue.tail.node.store(node, Relaxed);
            })
        }

        queue
    }

    fn push(&self, value: T) {
        epoch::pin(|scope| {
            let mut backoff = Backoff::new();
            loop {
                let tail = unsafe { self.tail.node.load(SeqCst, scope).deref() };
                let index = self.tail.index.load(SeqCst);
                let new_index = index.wrapping_add(1);
                let offset = index.wrapping_sub(tail.start);

                if offset >= NODE_LEN {
                    backoff.step();
                    continue;
                }

                if self.tail.index.compare_and_swap(index, new_index, SeqCst) == index {
                    unsafe {
                        let entry = tail.entries.get_unchecked(offset).get();
                        ptr::write(&mut (*entry).value, ManuallyDrop::new(value));
                        (*entry).ready.store(true, SeqCst);
                    }

                    if offset + 1 == NODE_LEN {
                        let new = Owned::new(Node::new(new_index)).into_ptr(scope);
                        tail.next.store(new, SeqCst);
                        self.tail.node.store(new, SeqCst);
                    }

                    return;
                }
            }
        })
    }

    fn pop(&self) -> Option<T> {
        epoch::pin(|scope| {
            let mut backoff = Backoff::new();
            loop {
                let head = self.head.node.load(SeqCst, scope);
                let h = unsafe { head.deref() };
                let index = self.head.index.load(SeqCst);
                let new_index = index.wrapping_add(1);
                let offset = index.wrapping_sub(h.start);

                if offset >= NODE_LEN {
                    backoff.step();
                    continue;
                }

                let entry = unsafe { &*h.entries.get_unchecked(offset).get() };

                if !entry.ready.load(Relaxed) && self.tail.index.load(SeqCst) == index {
                    return None;
                }

                if self.head.index.compare_and_swap(index, new_index, SeqCst) == index {
                    while !entry.ready.load(SeqCst) {
                        backoff.step();
                    }

                    if offset + 1 == NODE_LEN {
                        loop {
                            let next = h.next.load(SeqCst, scope);
                            if !next.is_null() {
                                self.head.node.store(next, SeqCst);
                                unsafe {
                                    scope.defer_free(head);
                                }
                                break;
                            }
                            backoff.step();
                        }
                    }

                    let v = unsafe { ptr::read(&(*entry).value) };
                    return Some(ManuallyDrop::into_inner(v));
                }
            }
        })
    }

    pub fn len(&self) -> usize {
        loop {
            let tail_index = self.tail.index.load(SeqCst);
            let head_index = self.head.index.load(SeqCst);

            if self.tail.index.load(SeqCst) == tail_index {
                return tail_index.wrapping_sub(head_index);
            }
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one();
            Ok(())
        }
    }

    pub fn send(&self, value: T) -> Result<(), SendTimeoutError<T>> {
        if self.closed.load(SeqCst) {
            Err(SendTimeoutError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one();
            Ok(())
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let closed = self.closed.load(SeqCst);
        match self.pop() {
            None => if closed {
                Err(TryRecvError::Disconnected)
            } else {
                Err(TryRecvError::Empty)
            },
            Some(v) => Ok(v),
        }
    }

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

            if is_closed {
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
        unsafe {
            epoch::unprotected(|scope| {
                let tail_index = self.tail.index.load(Relaxed);
                let mut head_index = self.head.index.load(Relaxed);
                let mut node = self.head.node.load(Relaxed, scope);

                while head_index != tail_index {
                    let n = node.deref();
                    let offset = head_index.wrapping_sub(n.start);
                    let entry = &mut *n.entries.get_unchecked(offset).get();

                    let v = ptr::read(&mut (*entry).value);
                    drop(ManuallyDrop::into_inner(v));

                    if offset + 1 == NODE_LEN {
                        let next = n.next.load(Relaxed, scope);
                        node.destroy();
                        node = next;
                    }

                    head_index = head_index.wrapping_add(1);
                }
            })
        }
    }
}
