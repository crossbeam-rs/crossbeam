use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, SeqCst};
use std::thread;
use std::time::{Instant, Duration};

use coco::epoch::{self, Atomic, Owned};

use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use impls::Channel;
use monitor::Monitor;

/// A single node in a queue.
struct Node<T> {
    /// The next node in the queue.
    next: Atomic<Node<T>>,
    /// The payload. TODO
    value: T,
}

/// A lock-free multi-producer multi-consumer queue.
#[repr(C)]
pub struct Queue<T> {
    /// Head of the queue.
    head: Atomic<Node<T>>,
    recvs: AtomicUsize,
    /// Some padding to avoid false sharing.
    _pad0: [u8; 64],
    /// Tail ofthe queue.
    tail: Atomic<Node<T>>,
    sends: AtomicUsize,
    /// Some padding to avoid false sharing.
    _pad1: [u8; 64],
    /// TODO
    closed: AtomicBool,
    receivers: Monitor,

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
            receivers: Monitor::new(),

            sends: AtomicUsize::new(0),
            recvs: AtomicUsize::new(0),

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

    fn push(&self, value: T) {
        let mut node = Owned::new(Node {
            value: value,
            next: Atomic::null(),
        });

        unsafe {
            epoch::unprotected(|scope| {
                let new = node.into_ptr(scope);
                let old = self.tail.swap(new, SeqCst, scope);
                self.sends.fetch_add(1, SeqCst);
                old.deref().next.store(new, SeqCst);
            })
        }
    }

    fn pop(&self) -> Option<T> {
        const USE: usize = 1;
        const MULTI: usize = 2;

        return unsafe {
            epoch::unprotected(|scope| unsafe {
                if self.head.load(Relaxed, scope).tag() & MULTI == 0 {
                    loop {
                        let head = self.head.fetch_or(USE, SeqCst, scope);
                        if head.tag() != 0 {
                            break;
                        }

                        let next = head.deref().next.load(SeqCst, scope);

                        if next.is_null() {
                            self.head.fetch_and(!USE, SeqCst, scope);

                            if self.tail.load(SeqCst, scope).as_raw() == head.as_raw() {
                                return None;
                            }

                            thread::yield_now();
                        } else {
                            let value = ptr::read(&next.deref().value);

                            if self.head
                                .compare_and_swap(head.with_tag(USE), next, SeqCst, scope)
                                .is_ok()
                            {
                                self.recvs.fetch_add(1, SeqCst);
                                Vec::from_raw_parts(head.as_raw() as *mut Node<T>, 0, 1);
                                return Some(value);
                            }
                            mem::forget(value);

                            self.head.fetch_and(!USE, SeqCst, scope);
                        }
                    }

                    self.head.fetch_or(MULTI, SeqCst, scope);
                    while self.head.load(SeqCst, scope).tag() & USE != 0 {
                        thread::yield_now();
                    }
                }

                epoch::pin(|scope| loop {
                    let head = self.head.load(SeqCst, scope);
                    let next = head.deref().next.load(SeqCst, scope);

                    if next.is_null() {
                        if self.tail.load(SeqCst, scope).as_raw() == head.as_raw() {
                            return None;
                        }
                    } else {
                        if self.head
                            .compare_and_swap(head, next.with_tag(MULTI), SeqCst, scope)
                            .is_ok()
                        {
                            self.recvs.fetch_add(1, SeqCst);
                            scope.defer_free(head);
                            return Some(ptr::read(&next.deref().value));
                        }
                    }

                    thread::yield_now();
                })
            })
        };
    }
}

impl<T> Channel<T> for Queue<T> {
    fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one();
            Ok(())
        }
    }

    fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        if self.closed.load(SeqCst) {
            Err(SendTimeoutError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one();
            Ok(())
        }
    }

    fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.pop() {
            None => {
                if self.closed.load(SeqCst) {
                    Err(TryRecvError::Disconnected)
                } else {
                    Err(TryRecvError::Empty)
                }
            }
            Some(v) => Ok(v),
        }
    }

    fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }

            let now = Instant::now();
            if let Some(end) = deadline {
                if now >= end {
                    return Err(RecvTimeoutError::Timeout);
                }
            }

            self.receivers.watch_start();

            match self.try_recv() {
                Ok(v) => {
                    self.receivers.watch_abort();
                    return Ok(v);
                }
                Err(TryRecvError::Disconnected) => {
                    self.receivers.watch_abort();
                    return Err(RecvTimeoutError::Disconnected);
                }
                Err(TryRecvError::Empty) => {
                    if let Some(end) = deadline {
                        thread::park_timeout(end - now);
                    } else {
                        thread::park();
                    }
                    self.receivers.watch_abort();
                }
            }
        }
    }

    fn len(&self) -> usize {
        unimplemented!()
    }

    fn is_empty(&self) -> usize {
        unimplemented!()
    }

    fn is_full(&self) -> usize {
        unimplemented!()
    }

    fn capacity(&self) -> Option<usize> {
        None
    }

    fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            return false;
        }

        self.receivers.notify_all();
        true
    }

    fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    fn monitor(&self) -> &Monitor {
        &self.receivers
    }

    fn is_ready(&self) -> bool {
        unimplemented!()
    }

    fn id(&self) -> usize {
        self as *const _ as usize
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        // TODO: impl Drop
        // TODO: debug_assert self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use crossbeam;

    use super::*;

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
                assert_eq!(q.try_recv(), Err(TryRecvError::Empty));
                thread::sleep(ms(150));
                assert_eq!(q.try_recv(), Ok(7));
                thread::sleep(ms(50));
                assert_eq!(q.try_recv(), Err(TryRecvError::Disconnected));
            });
            s.spawn(|| {
                thread::sleep(ms(100));
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

    #[test]
    fn close_signals_receiver() {
        let q = Queue::<()>::new();

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.recv(), Err(RecvError));
                assert!(q.is_closed());
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                q.close();
            });
        });
    }

    #[test]
    fn spsc() {
        const COUNT: usize = 100_000;

        let q = Queue::new();

        crossbeam::scope(|s| {
            s.spawn(|| {
                for i in 0..COUNT {
                    assert_eq!(q.recv(), Ok(i));
                }
                assert_eq!(q.recv(), Err(RecvError));
            });
            s.spawn(|| {
                for i in 0..COUNT {
                    q.send(i).unwrap();
                }
                q.close();
            });
        });
    }

    #[test]
    fn mpmc() {
        const COUNT: usize = 25_000;
        const THREADS: usize = 4;

        let q = Queue::<usize>::new();
        let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

        crossbeam::scope(|s| {
            for _ in 0..THREADS {
                s.spawn(|| for i in 0..COUNT {
                    let n = q.recv().unwrap();
                    v[n].fetch_add(1, SeqCst);
                });
            }
            for _ in 0..THREADS {
                s.spawn(|| for i in 0..COUNT {
                    q.send(i).unwrap();
                });
            }
        });

        for c in v {
            assert_eq!(c.load(SeqCst), THREADS);
        }
    }
}
