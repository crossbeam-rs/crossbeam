use std::cell::Cell;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fmt;
use std::marker::PhantomData;
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
use super::SendTimeoutError;
use super::RecvError;
use super::TryRecvError;
use super::RecvTimeoutError;

// TODO: Try Dmitry's modified MPSC queue instead of Michael-Scott. Moreover, don't use complex
// synchronization nor pinning if there's a single consumer. Note that Receiver can't be Sync in
// that case. Also, optimize the Sender side if there's only one.
// Note that in SPSC scenario the Receiver doesn't wait if the queue is in inconsistent state.

/// A single node in a queue.
struct Node<T> {
    /// The payload. TODO
    value: T,
    /// The next node in the queue.
    next: Atomic<Node<T>>,
}

/// A lock-free multi-producer multi-consumer queue.
#[repr(C)]
pub struct Queue<T> {
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

    receivers: Mutex<VecDeque<Thread>>,
    receivers_len: AtomicUsize,

    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn new() -> Self {
        // Initialize the internal representation of the queue.
        let queue = Queue {
            head: Atomic::null(),
            _pad0: unsafe { mem::uninitialized() },
            tail: Atomic::null(),
            _pad1: unsafe { mem::uninitialized() },
            closed: AtomicBool::new(false),
            receivers: Mutex::new(VecDeque::new()),
            receivers_len: AtomicUsize::new(0),
            _marker: PhantomData,
        };

        // Create a sentinel node.
        let node = Owned::new(Node {
            value: unsafe { mem::uninitialized() },
            next: Atomic::null(),
        });

        unsafe {
            epoch::unprotected(|scope| {
                let node = node.into_ptr(scope);
                queue.head.store(node, Relaxed);
                queue.tail.store(node, Relaxed);
            })
        }

        queue
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        let mut node = Owned::new(Node {
            value: value,
            next: Atomic::null(),
        });

        epoch::pin(|scope| {
            let mut tail = self.tail.load(Acquire, scope);

            loop {
                // Load the node following the tail.
                let t = unsafe { tail.deref() };
                let next = t.next.load(SeqCst, scope);

                match unsafe { next.as_ref() } {
                    None => {
                        // Try installing the new node.
                        match t.next.compare_and_swap_weak_owned(next, node, SeqCst, scope) {
                            Ok(node) => {
                                // Successfully pushed the node!
                                // Tail pointer mustn't fall behind. Move it forward.
                                let _ = self.tail.compare_and_swap(tail, node, AcqRel, scope);

                                if self.receivers_len.load(SeqCst) > 0 {
                                    let mut r = self.receivers.lock().unwrap();

                                    if let Some(t) = r.pop_front() {
                                        self.receivers_len.store(r.len(), SeqCst);
                                        t.unpark();
                                    }
                                }

                                return Ok(());
                            }
                            Err((next, n)) => {
                                // Failed. The node that actually follows `t` is `next`.
                                tail = next;
                                node = n;
                            }
                        }
                    }
                    Some(_) => {
                        // Tail pointer fell behind. Move it forward.
                        match self.tail.compare_and_swap_weak(tail, next, AcqRel, scope) {
                            Ok(()) => tail = next,
                            Err(t) => tail = t,
                        }
                    }
                }
            }
        })
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        match self.try_send(value) {
            Ok(()) => Ok(()),
            Err(TrySendError::Disconnected(v)) => Err(SendError(v)),
            Err(TrySendError::Full(_)) => unreachable!(),
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        epoch::pin(|scope| {
            let mut head = self.head.load(SeqCst, scope);

            loop {
                let next = unsafe { head.deref().next.load(SeqCst, scope) };

                match unsafe { next.as_ref() } {
                    None => {
                        if self.closed.load(SeqCst) {
                            return Err(TryRecvError::Disconnected);
                        } else {
                            return Err(TryRecvError::Empty);
                        }
                    }
                    Some(n) => {
                        // Try unlinking the head by moving it forward.
                        match self.head.compare_and_swap_weak(head, next, SeqCst, scope) {
                            Ok(_) => unsafe {
                                // The old head may be later freed.
                                scope.defer_free(head);

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

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {},
            }

            let now = Instant::now();
            if let Some(end) = deadline {
                if now >= end {
                    return Err(RecvTimeoutError::Timeout);
                }
            }

            // Register a receiver.
            {
                let mut r = self.receivers.lock().unwrap();
                r.push_back(thread::current());

                match self.try_recv() {
                    Ok(v) => {
                        r.pop_back();
                        return Ok(v);
                    }
                    Err(TryRecvError::Disconnected) => {
                        r.pop_back();
                        return Err(RecvTimeoutError::Disconnected);
                    }
                    Err(TryRecvError::Empty) => {}
                }

                self.receivers_len.store(r.len(), SeqCst);
            }

            if let Some(end) = deadline {
                thread::park_timeout(end - now);
            } else {
                thread::park();
            }

            let mut r = self.receivers.lock().unwrap();
            let id = thread::current().id();

            if let Some((i, _)) = r.iter().enumerate().find(|&(_, t)| t.id() == id) {
                r.remove(i);
            } else if let Some(t) = r.pop_front() {
                t.unpark();
            }

            self.receivers_len.store(r.len(), SeqCst);
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        self.recv_until(Some(Instant::now() + dur))
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        if let Ok(v) = self.recv_until(None) {
            Ok(v)
        } else {
            Err(RecvError)
        }
    }

    pub fn close(&self) -> bool {
        if !self.closed.swap(true, SeqCst) {
            let mut r = self.receivers.lock().unwrap();
            for t in r.drain(..) {
                t.unpark();
            }
            self.receivers_len.store(r.len(), SeqCst);

            true
        } else {
            false
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }
}

// TODO: impl Drop

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::thread;

    use crossbeam;

    // TODO: MPMC stress test
    // TODO: drop test

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn smoke() {
        let q = Queue::new();
        q.try_send(7).unwrap();
        assert_eq!(q.try_recv().unwrap(), 7);

        q.send(8);
        assert_eq!(q.recv().unwrap(), 8);

        assert_eq!(q.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(q.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn recv() {
        let q = Queue::new();

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.recv(), Ok(7));
                thread::sleep(ms(100));
                assert_eq!(q.recv(), Ok(8));
                thread::sleep(ms(100));
                assert_eq!(q.recv(), Ok(9));
                assert_eq!(q.recv(), Err(RecvError));
            });
            s.spawn(|| {
                thread::sleep(ms(150));
                assert_eq!(q.send(7), Ok(()));
                assert_eq!(q.send(8), Ok(()));
                assert_eq!(q.send(9), Ok(()));
                q.close();
            });
        });
    }

    #[test]
    fn recv_timeout() {
        let q = Queue::new();

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
                assert_eq!(q.recv_timeout(ms(100)), Ok(7));
                assert_eq!(q.recv_timeout(ms(100)), Err(RecvTimeoutError::Disconnected));
            });
            s.spawn(|| {
                thread::sleep(ms(150));
                assert_eq!(q.send(7), Ok(()));
                q.close();
            });
        });
    }

    #[test]
    fn try_recv() {
        let q = Queue::new();

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
                assert_eq!(q.recv_timeout(ms(100)), Ok(7));
                assert_eq!(q.recv_timeout(ms(100)), Err(RecvTimeoutError::Disconnected));
            });
            s.spawn(|| {
                thread::sleep(ms(150));
                assert_eq!(q.send(7), Ok(()));
                q.close();
            });
        });
    }

    #[test]
    fn is_closed() {
        let q = Queue::<()>::new();

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert!(!q.is_closed());
                thread::sleep(ms(150));
                assert!(q.is_closed());
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                assert!(!q.is_closed());
                q.close();
                assert!(q.is_closed());
            });
        });
    }

    #[test]
    fn recv_after_close() {
        let q = Queue::new();

        q.send(1).unwrap();
        q.send(2).unwrap();
        q.send(3).unwrap();

        q.close();

        assert_eq!(q.recv(), Ok(1));
        assert_eq!(q.recv(), Ok(2));
        assert_eq!(q.recv(), Ok(3));
        assert_eq!(q.recv(), Err(RecvError));
    }
}
