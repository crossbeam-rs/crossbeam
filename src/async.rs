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

use coco::epoch::{self, Atomic, Owned};
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
        // Initialize the internal representation of the queue.
        let inner = Inner {
            head: Atomic::null(),
            _pad0: unsafe { mem::uninitialized() },
            tail: Atomic::null(),
            _pad1: unsafe { mem::uninitialized() },
            closed: AtomicBool::new(false),
        };

        // Create a sentinel node.
        let node = Owned::new(Node {
            value: unsafe { mem::uninitialized() },
            next: Atomic::null(),
        });

        unsafe {
            epoch::unprotected(|scope| {
                let node = node.into_ptr(scope);
                inner.head.store(node, Relaxed);
                inner.tail.store(node, Relaxed);
            })
        }

        Queue(inner)
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let inner = &self.0;

        if inner.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        let mut node = Owned::new(Node {
            value: value,
            next: Atomic::null(),
        });

        epoch::pin(|scope| {
            let mut tail = inner.tail.load(Acquire, scope);

            loop {
                // Load the node following the tail.
                let t = unsafe { tail.deref() };
                let next = t.next.load(Acquire, scope);

                match unsafe { next.as_ref() } {
                    None => {
                        // Try installing the new node.
                        match t.next.compare_and_swap_weak_owned(next, node, AcqRel, scope) {
                            Ok(node) => {
                                // Successfully pushed the node!
                                // Tail pointer mustn't fall behind. Move it forward.
                                let _ = inner.tail.compare_and_swap(tail, node, AcqRel, scope);
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
                        match inner.tail.compare_and_swap_weak(tail, next, AcqRel, scope) {
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

        epoch::pin(|scope| {
            let mut head = inner.head.load(Acquire, scope);

            loop {
                let next = unsafe { head.deref().next.load(Acquire, scope) };

                match unsafe { next.as_ref() } {
                    None => {
                        if inner.closed.load(SeqCst) {
                            return Err(TryRecvError::Disconnected);
                        } else {
                            return Err(TryRecvError::Empty);
                        }
                    }
                    Some(n) => {
                        // Try unlinking the head by moving it forward.
                        match inner.head.compare_and_swap_weak(head, next, SeqCst, scope) { // TODO: SeqCst?
                            Ok(_) => unsafe {
                                // The old head may be later freed.
                                epoch::defer_free(head.as_raw(), 1, scope);

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

// TODO: impl Drop

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
