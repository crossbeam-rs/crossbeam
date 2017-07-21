use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Release, Relaxed, SeqCst};
use std::thread;
use std::time::{Instant, Duration};

use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use impls::Channel;
use monitor::Monitor;

// TODO: Should we use Acquire-Release or SeqCst

struct Node<T> {
    lap: AtomicUsize,
    value: T,
}

#[repr(C)]
pub struct Queue<T> {
    buffer: *mut UnsafeCell<Node<T>>,
    cap: usize,
    power: usize,
    _pad0: [u8; 64],
    head: AtomicUsize,
    _pad1: [u8; 64],
    tail: AtomicUsize,
    _pad2: [u8; 64],
    closed: AtomicBool,

    senders: Monitor,
    receivers: Monitor,

    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0);
        assert!(cap <= ::std::isize::MAX as usize);

        let power = cap.next_power_of_two();

        let mut v = Vec::with_capacity(cap);
        let buffer = v.as_mut_ptr();
        mem::forget(v);
        unsafe { ptr::write_bytes(buffer, 0, cap) }

        Queue {
            buffer: buffer,
            cap: cap,
            power: power,
            _pad0: unsafe { mem::uninitialized() },
            head: AtomicUsize::new(power),
            _pad1: unsafe { mem::uninitialized() },
            tail: AtomicUsize::new(0),
            _pad2: unsafe { mem::uninitialized() },
            closed: AtomicBool::new(false),

            senders: Monitor::new(),
            receivers: Monitor::new(),

            _marker: PhantomData,
        }
    }

    fn push(&self, value: T) -> Option<T> {
        let cap = self.cap;
        let power = self.power;
        let buffer = self.buffer;

        loop {
            let tail = self.tail.load(SeqCst);
            let pos = tail & (power - 1);
            let lap = tail & !(power - 1);

            let cell = unsafe { (*buffer.offset(pos as isize)).get() };
            let clap = unsafe { (*cell).lap.load(SeqCst) };

            if lap == clap {
                let new = if pos + 1 < cap {
                    tail + 1
                } else {
                    lap.wrapping_add(power).wrapping_add(power)
                };

                if self.tail
                    .compare_exchange_weak(tail, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    unsafe {
                        (*cell).value = value;
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return None;
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return Some(value);
            }

            thread::yield_now();
        }
    }

    fn pop(&self) -> Option<T> {
        let cap = self.cap;
        let power = self.power;
        let buffer = self.buffer;

        loop {
            let head = self.head.load(SeqCst);
            let pos = head & (power - 1);
            let lap = head & !(power - 1);

            let cell = unsafe { (*buffer.offset(pos as isize)).get() };
            let clap = unsafe { (*cell).lap.load(Acquire) };

            if lap == clap {
                let new = if pos + 1 < cap {
                    head + 1
                } else {
                    lap.wrapping_add(power).wrapping_add(power)
                };

                if self.head
                    .compare_exchange_weak(head, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    unsafe {
                        let value = ptr::read(&(*cell).value);
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Some(value);
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return None;
            }

            thread::yield_now();
        }
    }
}

impl<T> Channel<T> for Queue<T> {
    fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            match self.push(value) {
                None => {
                    self.receivers.notify_one();
                    Ok(())
                }
                Some(v) => Err(TrySendError::Full(v)),
            }
        }
    }

    fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            match self.try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(v)) => return Err(SendTimeoutError::Disconnected(v)),
                Err(TrySendError::Full(v)) => value = v,
            }

            let now = Instant::now();
            if let Some(end) = deadline {
                if now >= end {
                    return Err(SendTimeoutError::Timeout(value));
                }
            }

            self.senders.watch_start();

            match self.try_send(value) {
                Ok(()) => {
                    self.senders.watch_abort();
                    return Ok(());
                }
                Err(TrySendError::Disconnected(v)) => {
                    self.senders.watch_abort();
                    return Err(SendTimeoutError::Disconnected(v));
                }
                Err(TrySendError::Full(v)) => {
                    value = v;
                    if let Some(end) = deadline {
                        thread::park_timeout(end - now);
                    } else {
                        thread::park();
                    }
                    self.senders.watch_abort();
                }
            }
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
            Some(v) => {
                self.senders.notify_one();
                Ok(v)
            }
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

            if !self.is_ready() {
                if let Some(end) = deadline {
                    thread::park_timeout(end - now);
                } else {
                    thread::park();
                }
            }
            self.receivers.watch_abort();
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
        Some(self.cap)
    }

    fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            return false;
        }

        self.senders.notify_all();
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
        let power = self.power;
        let buffer = self.buffer;

        loop {
            let head = self.head.load(SeqCst);
            let pos = head & (power - 1);
            let lap = head & !(power - 1);

            let cell = unsafe { (*buffer.offset(pos as isize)).get() };
            let clap = unsafe { (*cell).lap.load(Acquire) };

            if lap == clap {
                return true;
            } else if clap.wrapping_add(power) == lap {
                return self.closed.load(SeqCst);
            }

            thread::yield_now();
        }
    }

    fn id(&self) -> usize {
        self as *const _ as usize
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        // TODO: impl Drop
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;

    use crossbeam;

    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    // TODO: drop test

    #[test]
    fn smoke() {
        let q = Queue::with_capacity(1);
        q.try_send(7).unwrap();
        assert_eq!(q.try_recv().unwrap(), 7);

        q.send(8);
        assert_eq!(q.recv().unwrap(), 8);

        assert_eq!(q.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(q.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn recv() {
        let q = Queue::with_capacity(100);

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
        let q = Queue::with_capacity(100);

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
        let q = Queue::with_capacity(100);

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
    fn send() {
        let q = Queue::with_capacity(1);

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.send(7), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(q.send(8), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(q.send(9), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(q.send(10), Err(SendError(10)));
            });
            s.spawn(|| {
                thread::sleep(ms(150));
                assert_eq!(q.recv(), Ok(7));
                assert_eq!(q.recv(), Ok(8));
                assert_eq!(q.recv(), Ok(9));
                q.close();
            });
        });
    }

    #[test]
    fn send_timeout() {
        let q = Queue::with_capacity(2);

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.send_timeout(1, ms(100)), Ok(()));
                assert_eq!(q.send_timeout(2, ms(100)), Ok(()));
                assert_eq!(q.send_timeout(3, ms(50)), Err(SendTimeoutError::Timeout(3)));
                thread::sleep(ms(100));
                assert_eq!(q.send_timeout(4, ms(100)), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(q.send(5), Err(SendError(5)));
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                assert_eq!(q.recv(), Ok(1));
                thread::sleep(ms(100));
                assert_eq!(q.recv(), Ok(2));
                assert_eq!(q.recv(), Ok(4));
                q.close();
            });
        });
    }

    #[test]
    fn try_send() {
        let q = Queue::with_capacity(1);

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.try_send(1), Ok(()));
                assert_eq!(q.try_send(2), Err(TrySendError::Full(2)));
                thread::sleep(ms(150));
                assert_eq!(q.try_send(3), Ok(()));
                thread::sleep(ms(50));
                assert_eq!(q.try_send(4), Err(TrySendError::Disconnected(4)));
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                assert_eq!(q.try_recv(), Ok(1));
                assert_eq!(q.try_recv(), Err(TryRecvError::Empty));
                assert_eq!(q.recv(), Ok(3));
                q.close();
            });
        });
    }

    #[test]
    fn is_closed() {
        let q = Queue::<()>::with_capacity(100);

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
        let q = Queue::with_capacity(100);

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
    fn close_signals_sender() {
        let q = Queue::with_capacity(1);

        crossbeam::scope(|s| {
            s.spawn(|| {
                assert_eq!(q.send(()), Ok(()));
                assert_eq!(q.send(()), Err(SendError(())));
                assert!(q.is_closed());
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                q.close();
            });
        });
    }

    #[test]
    fn close_signals_receiver() {
        let q = Queue::<()>::with_capacity(1);

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

        let q = Queue::with_capacity(3);

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

        let q = Queue::<usize>::with_capacity(3);
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
