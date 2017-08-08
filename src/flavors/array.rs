use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Release, Relaxed, SeqCst};
use std::thread;
use std::time::Instant;

use actor;
use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use monitor::Monitor;
use actor::HandleId;
use backoff::Backoff;

struct Node<T> {
    lap: AtomicUsize,
    value: T,
}

#[repr(C)]
pub struct Channel<T> {
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

impl<T> Channel<T> {
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0);
        assert!(cap <= ::std::isize::MAX as usize);

        let power = cap.next_power_of_two();

        let mut v = Vec::with_capacity(cap);
        let buffer = v.as_mut_ptr();
        mem::forget(v);

        unsafe {
            ptr::write_bytes(buffer, 0, cap);
        }

        Channel {
            buffer: buffer,
            cap: cap,
            power: power,
            head: AtomicUsize::new(power),
            tail: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            senders: Monitor::new(),
            receivers: Monitor::new(),
            _pad0: [0; 64],
            _pad1: [0; 64],
            _pad2: [0; 64],
            _marker: PhantomData,
        }
    }

    fn push(&self, value: T, backoff: &mut Backoff) -> Option<T> {
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

            if !backoff.tick() {
                thread::yield_now();
            }
        }
    }

    fn pop(&self, backoff: &mut Backoff) -> Option<T> {
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

            if !backoff.tick() {
                thread::yield_now();
            }
        }
    }

    pub fn len(&self) -> usize {
        let cap = self.cap;
        let power = self.power;

        loop {
            let tail = self.tail.load(SeqCst);
            let head = self.head.load(SeqCst);

            if self.tail.load(SeqCst) == tail {
                let tpos = tail & (power - 1);
                let tlap = tail & !(power - 1);

                let hpos = head & (power - 1);
                let hlap = head & !(power - 1);

                if hlap.wrapping_add(power) == tlap {
                    debug_assert!(cap - hpos + tpos <= cap);
                    return cap - hpos + tpos;
                } else {
                    debug_assert!(tpos >= hpos && tpos - hpos <= cap);
                    return tpos - hpos;
                }
            }
        }
    }

    pub fn try_send(
        &self,
        value: T,
    ) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            match self.push(value, &mut Backoff::new()) {
                None => {
                    self.receivers.notify_one();
                    Ok(())
                }
                Some(v) => Err(TrySendError::Full(v)),
            }
        }
    }

    pub fn spin_try_send(
        &self,
        mut value: T,
    ) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            let backoff = &mut Backoff::new();
            loop {
                match self.push(value, backoff) {
                    None => {
                        self.receivers.notify_one();
                        return Ok(());
                    }
                    Some(v) => value = v,
                }
                if !backoff.tick() {
                    break;
                }
            }
            Err(TrySendError::Full(value))
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            match self.spin_try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(v)) => value = v,
                Err(TrySendError::Disconnected(v)) => {
                    return Err(SendTimeoutError::Disconnected(v))
                }
            }

            actor::current().reset();
            self.senders.register(HandleId::sentinel());
            let timed_out = !self.is_closed() && self.len() == self.cap &&
                !actor::current().wait_until(deadline);
            self.senders.unregister(HandleId::sentinel());

            if timed_out {
                return Err(SendTimeoutError::Timeout(value));
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.pop(&mut Backoff::new()) {
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

    pub fn spin_try_recv(&self) -> Result<T, TryRecvError> {
        let backoff = &mut Backoff::new();
        loop {
            if let Some(v) = self.pop(backoff) {
                self.senders.notify_one();
                return Ok(v);
            }
            if !backoff.tick() {
                break;
            }
        }

        if self.closed.load(SeqCst) {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.spin_try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            }

            actor::current().reset();
            self.receivers.register(HandleId::sentinel());
            let timed_out =
                !self.is_closed() && self.len() == 0 && !actor::current().wait_until(deadline);
            self.receivers.unregister(HandleId::sentinel());

            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            false
        } else {
            self.senders.notify_all();
            self.receivers.notify_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    pub fn senders(&self) -> &Monitor {
        &self.senders
    }

    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        let cap = self.cap;
        let power = self.power;
        let buffer = self.buffer;

        let head = self.head.load(Relaxed);
        let mut pos = head & (power - 1);

        unsafe {
            for _ in 0..self.len() {
                let cell = (*buffer.offset(pos as isize)).get();
                ptr::drop_in_place(cell);

                pos += 1;
                if pos == self.cap {
                    pos = 0;
                }
            }

            Vec::from_raw_parts(buffer, 0, cap);
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

    use bounded;
    use err::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    // TODO: drop test

    #[test]
    fn smoke() {
        let (tx, rx) = bounded(1);
        tx.try_send(7).unwrap();
        assert_eq!(rx.try_recv().unwrap(), 7);

        tx.send(8);
        assert_eq!(rx.recv().unwrap(), 8);

        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(rx.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
    }

    #[test]
    fn recv() {
        let (tx, rx) = bounded(100);

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
        let (tx, rx) = bounded(100);

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
        let (tx, rx) = bounded(100);

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
    fn send() {
        let (tx, rx) = bounded(1);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(tx.send(7), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(tx.send(8), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(tx.send(9), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(tx.send(10), Err(SendError(10)));
            });
            s.spawn(move || {
                thread::sleep(ms(150));
                assert_eq!(rx.recv(), Ok(7));
                assert_eq!(rx.recv(), Ok(8));
                assert_eq!(rx.recv(), Ok(9));
            });
        });
    }

    #[test]
    fn send_timeout() {
        let (tx, rx) = bounded(2);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(tx.send_timeout(1, ms(100)), Ok(()));
                assert_eq!(tx.send_timeout(2, ms(100)), Ok(()));
                assert_eq!(
                    tx.send_timeout(3, ms(50)),
                    Err(SendTimeoutError::Timeout(3))
                );
                thread::sleep(ms(100));
                assert_eq!(tx.send_timeout(4, ms(100)), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(tx.send(5), Err(SendError(5)));
            });
            s.spawn(move || {
                thread::sleep(ms(100));
                assert_eq!(rx.recv(), Ok(1));
                thread::sleep(ms(100));
                assert_eq!(rx.recv(), Ok(2));
                assert_eq!(rx.recv(), Ok(4));
            });
        });
    }

    #[test]
    fn try_send() {
        let (tx, rx) = bounded(1);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(tx.try_send(1), Ok(()));
                assert_eq!(tx.try_send(2), Err(TrySendError::Full(2)));
                thread::sleep(ms(150));
                assert_eq!(tx.try_send(3), Ok(()));
                thread::sleep(ms(50));
                assert_eq!(tx.try_send(4), Err(TrySendError::Disconnected(4)));
            });
            s.spawn(move || {
                thread::sleep(ms(100));
                assert_eq!(rx.try_recv(), Ok(1));
                assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
                assert_eq!(rx.recv(), Ok(3));
            });
        });
    }

    #[test]
    fn recv_after_close() {
        let (tx, rx) = bounded(100);

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
    fn close_signals_sender() {
        let (tx, rx) = bounded(1);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(tx.send(()), Ok(()));
                assert_eq!(tx.send(()), Err(SendError(())));
            });
            s.spawn(move || {
                thread::sleep(ms(100));
                drop(rx);
            });
        });
    }

    #[test]
    fn close_signals_receiver() {
        let (tx, rx) = bounded::<()>(1);

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

        let (tx, rx) = bounded(3);

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

        let (tx, rx) = bounded::<usize>(3);
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
