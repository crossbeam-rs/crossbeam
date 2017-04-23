use std::cell::Cell;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize, fence};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use coco::epoch;
use coco::epoch::Atomic;
use either::Either;

use super::SendError;
use super::TrySendError;
use super::RecvError;
use super::TryRecvError;

/// A single node in a queue.
struct Node<T> {
    /// The payload. TODO
    value: T,
    /// The next node in the queue.
    next: Atomic<Node<T>>,
}

/// The inner representation of a queue.
///
/// It consists of a head and tail pointer, with some padding in-between to avoid false sharing.
/// A queue is a singly linked list of value nodes. There is always one sentinel value node, and
/// that is the head. If both head and tail point to the sentinel node, the queue is empty.
///
/// If the queue is empty, there might be a list of request nodes following the tail, which
/// represent threads blocked on future push operations. The tail never moves onto a request node.
///
/// To summarize, the structure of the queue is always one of these two:
///
/// 1. Sentinel node (head), followed by a number of value nodes (the last one is tail).
/// 2. Sentinel node (head and tail), followed by a number of request nodes.
///
/// Requests are fulfilled by marking the next-pointer of it's node, then copying a value into the
/// slot, and finally signalling that the blocked thread is ready to be woken up. Nodes with marked
/// next-pointers are considered to be deleted and can always be unlinked from the list. A request
/// can cancel itself simply by marking the next-pointer of it's node.
#[repr(C)]
struct Inner<T> {
    /// Head of the queue.
    head: Atomic<Node<T>>,
    /// Some padding to avoid false sharing.
    _pad0: [u8; 64],
    /// Tail ofthe queue.
    tail: Atomic<Node<T>>,
    /// Some padding to avoid false sharing.
    _pad1: [u8; 64],
    /// TODO
    closed: AtomicBool,
}

/// A lock-free multi-producer multi-consumer queue.
pub struct Queue<T>(Inner<T>);

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn new() -> Self {
        // Create a sentinel node.
        let node = Box::new(Node {
            value: unsafe { mem::uninitialized() },
            next: Atomic::null(0),
        });

        // Initialize the internal representation of the queue.
        let inner = Inner {
            head: Atomic::from_box(node, 0),
            _pad0: unsafe { mem::uninitialized() },
            tail: Atomic::null(0),
            _pad1: unsafe { mem::uninitialized() },
            closed: AtomicBool::new(false),
        };

        // Copy the head pointer into the tail pointer.
        epoch::pin(|pin| inner.tail.store(inner.head.load(pin)));

        Queue(inner)
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let inner = &self.0;

        if inner.closed.load(SeqCst) {
            return Err(TrySendError::Closed(value));
        }

        let mut node = Box::new(Node {
            value: value,
            next: Atomic::null(0),
        });

        epoch::pin(|pin| {
            let mut tail = inner.tail.load(pin);

            loop {
                // Load the node following the tail.
                let t = tail.unwrap();
                let next = t.next.load(pin);

                match next.as_ref() {
                    None => {
                        // Try installing the new node.
                        match t.next.cas_box_weak(next, node, 0) {
                            Ok(node) => {
                                // Successfully pushed the node!
                                // Tail pointer mustn't fall behind. Move it forward.
                                let _ = inner.tail.cas(tail, node);
                                return Ok(());
                            }
                            Err((next, n)) => {
                                // Failed. The node that actually follows `t` is `next`.
                                tail = next;
                                node = n;
                            }
                        }
                    }
                    Some(n) => {
                        // Tail pointer fell behind. Move it forward.
                        match inner.tail.cas_weak(tail, next) {
                            Ok(()) => tail = next,
                            Err(t) => tail = t,
                        }
                    }
                }
            }
        })
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let inner = &self.0;

        if inner.closed.load(SeqCst) {
            return Err(TryRecvError::Closed);
        }

        epoch::pin(|pin| {
            let mut head = inner.head.load(pin);
            loop {
                let next = head.unwrap().next.load(pin);
                match next.as_ref() {
                    None => return Err(TryRecvError::Empty),
                    Some(n) => {
                        // Try unlinking the head by moving it forward.
                        match inner.head.cas_weak_sc(head, next) {
                            Ok(_) => unsafe {
                                // The old head may be later freed.
                                epoch::defer_free(head.as_raw(), 1, pin);

                                // The new head holds the popped value (heads are sentinels!).
                                return Ok(ptr::read(&n.value));
                            },
                            Err(h) => head = h,
                        }
                    }
                }
            }
        })
    }

    pub fn close(&self) -> bool {
        self.0.closed.swap(true, SeqCst) == false
    }

    pub fn is_closed(&self) -> bool {
        self.0.closed.load(SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::thread;

    #[test]
    fn simple() {
        const STEPS: usize = 1_000_000;

        let q = Arc::new(Queue::with_capacity(5));

        let t = {
            let q = q.clone();
            thread::spawn(move || {
                for i in 0..STEPS {
                    q.send(5);
                }
                println!("SEND DONE");
            })
        };

        for _ in 0..STEPS {
            q.recv();
        }
        println!("RECV DONE");

        t.join().unwrap();
    }
}
