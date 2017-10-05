use std::cell::UnsafeCell;
use std::cmp;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::thread;
use std::time::Instant;

use coco::epoch::{self, Atomic, Owned};

use CaseId;
use actor;
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

#[repr(C)]
pub(crate) struct Channel<T> {
    low: AtomicUsize,
    head: Atomic<Node<T>>,
    _pad0: [u8; 64],
    high: AtomicUsize,
    tail: Atomic<Node<T>>,
    _pad1: [u8; 64],
    closed: AtomicBool,
    receivers: Monitor,
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        let queue = Channel {
            head: Atomic::null(),
            tail: Atomic::null(),
            low: AtomicUsize::new(0),
            high: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            receivers: Monitor::new(),
            _pad0: [0; 64],
            _pad1: [0; 64],
            _marker: PhantomData,
        };

        unsafe {
            epoch::unprotected(|scope| {
                let node = Owned::new(Node::new(0)).into_ptr(scope);
                queue.head.store(node, Relaxed);
                queue.tail.store(node, Relaxed);
            })
        }

        queue
    }

    fn push(&self, value: T) {
        epoch::pin(|scope| {
            loop {
                let tail = unsafe { self.tail.load(SeqCst, scope).deref() };
                let high = self.high.load(SeqCst);
                let new_high = high.wrapping_add(1);
                let offset = high.wrapping_sub(tail.start);

                if offset >= NODE_LEN {
                    thread::yield_now();
                    continue;
                }

                if self.high.compare_and_swap(high, new_high, SeqCst) == high {
                    unsafe {
                        let entry = tail.entries.get_unchecked(offset).get();
                        ptr::write(&mut (*entry).value, ManuallyDrop::new(value));
                        (*entry).ready.store(true, SeqCst);
                    }

                    if offset + 1 == NODE_LEN {
                        let new = Owned::new(Node::new(new_high)).into_ptr(scope);
                        tail.next.store(new, SeqCst);
                        self.tail.store(new, SeqCst);
                    }

                    return;
                }
            }
        })
    }

    fn pop(&self) -> Option<T> {
        epoch::pin(|scope| {
            loop {
                let head = self.head.load(SeqCst, scope);
                let h = unsafe { head.deref() };
                let low = self.low.load(SeqCst);
                let new_low = low.wrapping_add(1);
                let offset = low.wrapping_sub(h.start);

                if offset >= NODE_LEN {
                    thread::yield_now();
                    continue;
                }

                let entry = unsafe { &*h.entries.get_unchecked(offset).get() };

                if !entry.ready.load(Relaxed) && self.high.load(SeqCst) == low {
                    return None;
                }

                if self.low.compare_and_swap(low, new_low, SeqCst) == low {
                    while !entry.ready.load(SeqCst) {
                        thread::yield_now();
                    }

                    if offset + 1 == NODE_LEN {
                        loop {
                            let next = h.next.load(SeqCst, scope);
                            if !next.is_null() {
                                self.head.store(next, SeqCst);
                                unsafe {
                                    scope.defer_free(head);
                                }
                                break;
                            }
                            thread::yield_now();
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
            let high = self.high.load(SeqCst);
            let low = self.low.load(SeqCst);

            if self.high.load(SeqCst) == high {
                return high.wrapping_sub(low);
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
            let timed_out =
                !self.is_closed() && self.len() == 0 && !actor::current_wait_until(deadline);
            self.receivers.unregister(case_id);

            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    pub fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            false
        } else {
            self.receivers.abort_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        unsafe {
            epoch::unprotected(|scope| {
                let high = self.high.load(Relaxed);
                let mut low = self.low.load(Relaxed);
                let mut node = self.head.load(Relaxed, scope);

                while low != high {
                    let n = node.deref();
                    let offset = low.wrapping_sub(n.start);
                    let entry = &mut *n.entries.get_unchecked(offset).get();

                    let v = ptr::read(&mut (*entry).value);
                    drop(ManuallyDrop::into_inner(v));

                    if offset + 1 == NODE_LEN {
                        let next = n.next.load(Relaxed, scope);
                        node.destroy();
                        node = next;
                    }

                    low = low.wrapping_add(1);
                }
            })
        }
    }
}
