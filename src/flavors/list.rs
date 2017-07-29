use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, SeqCst};
use std::thread;
use std::time::{Instant, Duration};

use coco::epoch::{self, Atomic, Owned};

use actor;
use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use watch::monitor::Monitor;

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
    recv_count: AtomicUsize,
    /// Some padding to avoid false sharing.
    _pad0: [u8; 64],
    /// Tail ofthe queue.
    tail: Atomic<Node<T>>,
    send_count: AtomicUsize,
    /// Some padding to avoid false sharing.
    _pad1: [u8; 64],
    /// TODO
    closed: AtomicBool,
    receivers: Monitor,
    _marker: PhantomData<T>,
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        // Initialize the internal representation of the queue.
        let queue = Queue {
            _pad0: [0; 64],
            _pad1: [0; 64],
            head: Atomic::null(),
            tail: Atomic::null(),
            closed: AtomicBool::new(false),
            receivers: Monitor::new(),
            send_count: AtomicUsize::new(0),
            recv_count: AtomicUsize::new(0),
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
                self.send_count.fetch_add(1, SeqCst);
                old.deref().next.store(new, SeqCst);
            })
        }
    }

    fn pop(&self) -> Option<T> {
        const USE: usize = 1;
        const MULTI: usize = 2;

        // TODO: finer grained unsafe code
        return unsafe {
            epoch::unprotected(|scope| {
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
                                self.recv_count.fetch_add(1, SeqCst);
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
                            self.recv_count.fetch_add(1, SeqCst);
                            scope.defer_free(head);
                            return Some(ptr::read(&next.deref().value));
                        }
                    }

                    thread::yield_now();
                })
            })
        };
    }

    pub fn len(&self) -> usize {
        loop {
            let send_count = self.send_count.load(SeqCst);
            let recv_count = self.recv_count.load(SeqCst);

            if self.send_count.load(SeqCst) == send_count {
                return send_count.wrapping_sub(recv_count);
            }

            thread::yield_now();
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one(self.id());
            Ok(())
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        if self.closed.load(SeqCst) {
            Err(SendTimeoutError::Disconnected(value))
        } else {
            self.push(value);
            self.receivers.notify_one(self.id());
            Ok(())
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
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

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
            }

            actor::current().reset();
            self.receivers.register();
            let timed_out =
                !self.is_closed() && self.len() == 0 && !actor::current().wait_until(deadline);
            self.receivers.unregister();

            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    pub fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            false
        } else {
            self.receivers.notify_all(self.id());
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    pub fn monitor_rx(&self) -> &Monitor {
        &self.receivers
    }

    pub fn id(&self) -> usize {
        self as *const _ as usize
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        unsafe {
            epoch::unprotected(|scope| {
                let mut head = self.head.load(Relaxed, scope);
                while !head.is_null() {
                    let next = head.deref().next.load(Relaxed, scope);

                    if let Some(n) = next.as_ref() {
                        ptr::drop_in_place(&n.value as *const _ as *mut Node<T>)
                    }

                    Vec::from_raw_parts(head.as_raw() as *mut Node<T>, 0, 1);
                    head = next;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;
    use std::time::{Instant, Duration};

    use crossbeam;

    use unbounded;
    use err::*;

    // TODO: drop test

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn smoke() {
        let (tx, rx) = unbounded();
        tx.try_send(7).unwrap();
        assert_eq!(rx.try_recv().unwrap(), 7);

        tx.send(8);
        assert_eq!(rx.recv().unwrap(), 8);

        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(rx.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn recv() {
        let (tx, rx) = unbounded();

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(rx.recv(), Ok(7));
                thread::sleep(ms(100));
                assert_eq!(rx.recv(), Ok(8));
                thread::sleep(ms(100));
                assert_eq!(rx.recv(), Ok(9));
                assert_eq!(rx.recv(), Err(RecvError));
            });
            s.spawn(move || {
                thread::sleep(ms(150));
                assert_eq!(tx.send(7), Ok(()));
                assert_eq!(tx.send(8), Ok(()));
                assert_eq!(tx.send(9), Ok(()));
            });
        });
    }

    #[test]
    fn recv_timeout() {
        let (tx, rx) = unbounded();

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(rx.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
                assert_eq!(rx.recv_timeout(ms(100)), Ok(7));
                assert_eq!(
                    rx.recv_timeout(ms(100)),
                    Err(RecvTimeoutError::Disconnected)
                );
            });
            s.spawn(move || {
                thread::sleep(ms(150));
                assert_eq!(tx.send(7), Ok(()));
            });
        });
    }

    #[test]
    fn try_recv() {
        let (tx, rx) = unbounded();

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
                thread::sleep(ms(150));
                assert_eq!(rx.try_recv(), Ok(7));
                thread::sleep(ms(50));
                assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
            });
            s.spawn(move || {
                thread::sleep(ms(100));
                assert_eq!(tx.send(7), Ok(()));
            });
        });
    }

    #[test]
    fn recv_after_close() {
        let (tx, rx) = unbounded();

        tx.send(1).unwrap();
        tx.send(2).unwrap();
        tx.send(3).unwrap();

        drop(tx);

        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(rx.recv(), Ok(2));
        assert_eq!(rx.recv(), Ok(3));
        assert_eq!(rx.recv(), Err(RecvError));
    }

    #[test]
    fn close_signals_receiver() {
        let (tx, rx) = unbounded::<()>();

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(rx.recv(), Err(RecvError));
            });
            s.spawn(move || {
                thread::sleep(ms(100));
                drop(tx);
            });
        });
    }

    #[test]
    fn spsc() {
        const COUNT: usize = 100_000;

        let (tx, rx) = unbounded();

        crossbeam::scope(|s| {
            s.spawn(move || {
                for i in 0..COUNT {
                    assert_eq!(rx.recv(), Ok(i));
                }
                assert_eq!(rx.recv(), Err(RecvError));
            });
            s.spawn(move || for i in 0..COUNT {
                tx.send(i).unwrap();
            });
        });
    }

    #[test]
    fn mpmc() {
        const COUNT: usize = 25_000;
        const THREADS: usize = 4;

        let (tx, rx) = unbounded::<usize>();
        let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

        crossbeam::scope(|s| {
            for _ in 0..THREADS {
                s.spawn(|| for i in 0..COUNT {
                    let n = rx.recv().unwrap();
                    v[n].fetch_add(1, SeqCst);
                });
            }
            for _ in 0..THREADS {
                s.spawn(|| for i in 0..COUNT {
                    tx.send(i).unwrap();
                });
            }
        });

        for c in v {
            assert_eq!(c.load(SeqCst), THREADS);
        }
    }
}
