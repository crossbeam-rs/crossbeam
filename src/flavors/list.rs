use std::cell::UnsafeCell;
use std::cmp;
use std::marker::PhantomData;
use std::mem;
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
    value: T,
    ready: AtomicBool,
}

#[repr(C)]
struct Node<T> {
    seq: usize,
    low: AtomicUsize,
    entries: [UnsafeCell<Entry<T>>; NODE_LEN],
    high: AtomicUsize,
    next: Atomic<Node<T>>,
}

impl<T> Node<T> {
    fn new(seq: usize) -> Node<T> {
        let node = Node {
            seq,
            entries: unsafe { mem::uninitialized() },
            low: AtomicUsize::new(0),
            high: AtomicUsize::new(0),
            next: Atomic::null(),
        };
        for entry in node.entries.iter() {
            unsafe {
                (*entry.get()).ready = AtomicBool::new(false);
            }
        }
        node
    }
}

#[repr(C)]
pub(crate) struct Channel<T> {
    head: Atomic<Node<T>>,
    _pad0: [u8; 64],
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

                if tail.high.load(SeqCst) >= NODE_LEN {
                    thread::yield_now();
                    continue;
                }
                let i = tail.high.fetch_add(1, SeqCst);
                unsafe {
                    if i < NODE_LEN {
                        let entry = tail.entries.get_unchecked(i).get();
                        ptr::write(&mut (*entry).value, value);
                        (*entry).ready.store(true, SeqCst);

                        if i + 1 == NODE_LEN {
                            let new_seq = tail.seq.wrapping_add(1);
                            let new = Owned::new(Node::new(new_seq)).into_ptr(scope);
                            tail.next.store(new, SeqCst);
                            self.tail.store(new, SeqCst);
                        }

                        return;
                    }
                }
                thread::yield_now();
            }
        })
    }

    fn pop(&self) -> Option<T> {
        epoch::pin(|scope| {
            loop {
                let head = self.head.load(SeqCst, scope);
                let h = unsafe { head.deref() };

                loop {
                    let low = h.low.load(SeqCst);
                    if low >= cmp::min(h.high.load(SeqCst), NODE_LEN) {
                        thread::yield_now();
                        break
                    }
                    if h.low.compare_and_swap(low, low+1, SeqCst) == low {
                        unsafe {
                            let entry = h.entries.get_unchecked(low).get();
                            let mut i = 0;
                            loop {
                                if (*entry).ready.load(SeqCst) { break }
                                i += 1;
                                if i >= 50 { // TODO
                                    thread::yield_now();
                                }
                            }
                            if low + 1 == NODE_LEN {
                                loop {
                                    let next = h.next.load(SeqCst, scope);
                                    if !next.is_null() {
                                        self.head.store(next, SeqCst);
                                        scope.defer_free(head);
                                        break;
                                    }
                                    thread::yield_now();
                                }
                            }
                            return Some(ptr::read(&(*entry).value))
                        }
                    }
                    thread::yield_now();
                }
                if h.next.load(SeqCst, scope).is_null() { return None }
            }
        })
    }

    pub fn len(&self) -> usize {
        epoch::pin(|scope| {
            loop {
                let head = self.head.load(SeqCst, scope);
                let tail = self.tail.load(SeqCst, scope);

                let h = unsafe { head.deref() };
                let t = unsafe { tail.deref() };

                let low = h.low.load(SeqCst);
                let high = t.high.load(SeqCst);

                if self.head.load(SeqCst, scope).as_raw() == head.as_raw() &&
                    self.tail.load(SeqCst, scope).as_raw() == tail.as_raw() &&
                    low == h.low.load(SeqCst) &&
                    high == t.high.load(SeqCst)
                {
                    let low = cmp::min(low, NODE_LEN);
                    let high = cmp::min(high, NODE_LEN);
                    let node_count = t.seq.wrapping_sub(h.seq);
                    return node_count * NODE_LEN + high - low;
                }
            }
        })
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
                let mut head = self.head.load(Relaxed, scope);
                loop {
                    match head.as_ref() {
                        None => break,
                        Some(h) => {
                            let low = h.low.load(Relaxed);
                            let high = h.high.load(Relaxed);
                            let next = h.next.load(Relaxed, scope);

                            for i in low..cmp::min(high, NODE_LEN) {
                                ptr::drop_in_place(head.deref().entries.get_unchecked(i).get());
                            }

                            Vec::from_raw_parts(head.as_raw() as *mut Node<T>, 0, 1);
                            head = next;
                        }
                    }
                }
            })
        }
    }
}
