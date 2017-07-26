use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};
use std::time::Instant;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use impls::Channel;
use monitor::Monitor;
use Participant;
use PARTICIPANT;
use Wait;

struct Blocked<T> {
    participant: Arc<Participant>,
    data: Option<*const UnsafeCell<Option<T>>>,
}

struct Inner<T> {
    senders: VecDeque<Blocked<T>>,
    receivers: VecDeque<Blocked<T>>,
}

pub struct Queue<T> {
    lock: Mutex<Inner<T>>,
    closed: AtomicBool,
    senders_len: AtomicUsize,
    receivers_len: AtomicUsize,
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            lock: Mutex::new(Inner {
                senders: VecDeque::new(),
                receivers: VecDeque::new(),
            }),
            closed: AtomicBool::new(false),
            senders_len: AtomicUsize::new(0),
            receivers_len: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    pub fn promise_recv(&self) {
        let mut lock = self.lock.lock().unwrap();

        lock.receivers.push_back(Blocked {
            participant: PARTICIPANT.with(|p| p.clone()),
            data: None,
        });
        self.receivers_len.store(lock.receivers.len(), SeqCst);
    }

    pub fn unpromise_recv(&self) {
        let mut lock = self.lock.lock().unwrap();
        PARTICIPANT.with(|p| {
            lock.receivers.retain(|s| !ptr::eq(&*s.participant, &**p));
        });
        self.receivers_len.store(lock.receivers.len(), SeqCst);
    }
}

impl<T> Channel<T> for Queue<T> {
    fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        if self.receivers_len.load(SeqCst) == 0 {
            return Err(TrySendError::Full(value));
        }

        let mut lock = self.lock.lock().unwrap();

        if self.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        while let Some(f) = lock.receivers.pop_front() {
            self.receivers_len.store(lock.receivers.len(), SeqCst);
            unsafe {
                match f.data {
                    None => {
                        if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                            let wait = Wait {
                                participant: PARTICIPANT.with(|p| p.clone()),
                                data: UnsafeCell::new(Some(value)),
                            };
                            PARTICIPANT.with(|p| p.sel.store(0, SeqCst));
                            f.participant.ptr.store(&wait as *const _ as usize, SeqCst);
                            drop(lock);

                            while PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 0 {
                                thread::park();
                            }
                            self.lock.lock().unwrap();
                            return Ok(());
                        }
                    }
                    Some(data) => {
                        if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                            *(*data).get().as_mut().unwrap() = Some(value);
                            f.participant.thread.unpark();
                            return Ok(());
                        }
                    }
                }
            }
        }

        Err(TrySendError::Full(value))
    }

    fn send_until(&self, value: T, deadline: Option<Instant>) -> Result<(), SendTimeoutError<T>> {
        if self.closed.load(SeqCst) {
            return Err(SendTimeoutError::Disconnected(value));
        }

        let cell;
        {
            let mut lock = self.lock.lock().unwrap();

            if self.closed.load(SeqCst) {
                return Err(SendTimeoutError::Disconnected(value));
            }

            while let Some(f) = lock.receivers.pop_front() {
                self.receivers_len.store(lock.receivers.len(), SeqCst);
                unsafe {
                    match f.data {
                        None => {
                            if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                                let wait = Wait {
                                    participant: PARTICIPANT.with(|p| p.clone()),
                                    data: UnsafeCell::new(Some(value)),
                                };
                                PARTICIPANT.with(|p| p.sel.store(0, SeqCst));
                                f.participant.ptr.store(&wait as *const _ as usize, SeqCst);
                                drop(lock);

                                while PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 0 {
                                    thread::park();
                                }
                                self.lock.lock().unwrap();
                                return Ok(());
                            }
                        }
                        Some(data) => {
                            if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                                *(*data).get().as_mut().unwrap() = Some(value);
                                f.participant.thread.unpark();
                                return Ok(());
                            }
                        }
                    }
                }
            }

            PARTICIPANT.with(|p| p.sel.store(0, SeqCst));
            cell = UnsafeCell::new(Some(value));
            lock.senders.push_back(Blocked {
                participant: PARTICIPANT.with(|p| p.clone()),
                data: Some(&cell),
            });
            self.senders_len.store(lock.senders.len(), SeqCst);
        }

        while PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 0 {
            let now = Instant::now();
            if let Some(end) = deadline {
                if now < end {
                    thread::park_timeout(end - now);
                } else if PARTICIPANT.with(|p| p.sel.compare_and_swap(0, 1, SeqCst)) == 0 {
                    let mut lock = self.lock.lock().unwrap();

                    lock.senders
                        .retain(|s| s.data.map_or(true, |x| !ptr::eq(x, &cell)));
                    self.senders_len.store(lock.senders.len(), SeqCst);

                    let v = unsafe { cell.get().as_mut().unwrap().take().unwrap() };
                    return Err(SendTimeoutError::Timeout(v));
                }
            } else {
                thread::park();
            }
        }

        if PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 1 {
            let v = unsafe { cell.get().as_mut().unwrap().take().unwrap() };
            return Err(SendTimeoutError::Disconnected(v));
        }

        self.lock.lock().unwrap();
        Ok(())
    }

    fn try_recv(&self) -> Result<T, TryRecvError> {
        if self.closed.load(SeqCst) {
            return Err(TryRecvError::Disconnected);
        }

        if self.senders_len.load(SeqCst) == 0 {
            return Err(TryRecvError::Empty);
        }

        let mut lock = self.lock.lock().unwrap();

        if self.closed.load(SeqCst) {
            return Err(TryRecvError::Disconnected);
        }

        while let Some(f) = lock.senders.pop_front() {
            self.senders_len.store(lock.senders.len(), SeqCst);
            unsafe {
                match f.data {
                    None => {
                        if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                            let wait = Wait {
                                participant: PARTICIPANT.with(|p| p.clone()),
                                data: UnsafeCell::new(None),
                            };
                            PARTICIPANT.with(|p| p.sel.store(0, SeqCst));
                            f.participant.ptr.store(&wait as *const _ as usize, SeqCst);
                            drop(lock);

                            while PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 0 {
                                thread::park();
                            }
                            self.lock.lock().unwrap();
                            let v = wait.data.get().as_mut().unwrap().take().unwrap();
                            return Ok(v);
                        }
                    }
                    Some(data) => {
                        if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                            let v = (*data).get().as_mut().unwrap().take().unwrap();
                            f.participant.thread.unpark();
                            return Ok(v);
                        }
                    }
                }
            }
        }

        Err(TryRecvError::Empty)
    }

    fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        if self.closed.load(SeqCst) {
            return Err(RecvTimeoutError::Disconnected);
        }

        let cell;
        {
            let mut lock = self.lock.lock().unwrap();

            if self.closed.load(SeqCst) {
                return Err(RecvTimeoutError::Disconnected);
            }

            while let Some(f) = lock.senders.pop_front() {
                self.senders_len.store(lock.senders.len(), SeqCst);
                unsafe {
                    match f.data {
                        None => {
                            if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                                let wait = Wait {
                                    participant: PARTICIPANT.with(|p| p.clone()),
                                    data: UnsafeCell::new(None),
                                };
                                PARTICIPANT.with(|p| p.sel.store(0, SeqCst));
                                f.participant.ptr.store(&wait as *const _ as usize, SeqCst);
                                drop(lock);

                                while PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 0 {
                                    thread::park();
                                }
                                self.lock.lock().unwrap();
                                let v = wait.data.get().as_mut().unwrap().take().unwrap();
                                return Ok(v);
                            }
                        }
                        Some(data) => {
                            if f.participant.sel.compare_and_swap(0, self.id(), SeqCst) == 0 {
                                let v = (*data).get().as_mut().unwrap().take().unwrap();
                                f.participant.thread.unpark();
                                return Ok(v);
                            }
                        }
                    }
                }
            }

            PARTICIPANT.with(|p| p.sel.store(0, SeqCst));
            cell = UnsafeCell::new(None);
            lock.receivers.push_back(Blocked {
                participant: PARTICIPANT.with(|p| p.clone()),
                data: Some(&cell),
            });
            self.receivers_len.store(lock.receivers.len(), SeqCst);
        }

        while PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 0 {
            let now = Instant::now();
            if let Some(end) = deadline {
                if now < end {
                    thread::park_timeout(end - now);
                } else if PARTICIPANT.with(|p| p.sel.compare_and_swap(0, 1, SeqCst)) == 0 {
                    let mut lock = self.lock.lock().unwrap();

                    lock.receivers
                        .retain(|s| s.data.map_or(true, |x| !ptr::eq(x, &cell)));
                    self.receivers_len.store(lock.receivers.len(), SeqCst);

                    return Err(RecvTimeoutError::Timeout);
                }
            } else {
                thread::park();
            }
        }

        if PARTICIPANT.with(|p| p.sel.load(SeqCst)) == 1 {
            return Err(RecvTimeoutError::Disconnected);
        }

        self.lock.lock().unwrap();
        let v = unsafe { cell.get().as_mut().unwrap().take().unwrap() };
        Ok(v)
    }

    fn len(&self) -> usize {
        0
    }

    fn is_empty(&self) -> bool {
        let _lock = self.lock.lock().unwrap();
        self.senders_len.load(SeqCst) == 0
    }

    fn is_full(&self) -> bool {
        let _lock = self.lock.lock().unwrap();
        self.receivers_len.load(SeqCst) == 0
    }

    fn capacity(&self) -> Option<usize> {
        Some(0)
    }

    fn close(&self) -> bool {
        if self.closed.load(SeqCst) {
            return false;
        }

        let mut lock = self.lock.lock().unwrap();

        if self.closed.swap(true, SeqCst) {
            return false;
        }

        self.senders_len.store(0, SeqCst);
        self.receivers_len.store(0, SeqCst);

        for t in lock.senders.drain(..) {
            unsafe {
                match t.data {
                    None => {
                        unimplemented!();
                    }
                    Some(data) => {
                        if t.participant.sel.compare_and_swap(0, 1, SeqCst) == 0 {
                            t.participant.thread.unpark();
                        }
                    }
                }
            }
        }

        for t in lock.receivers.drain(..) {
            unsafe {
                match t.data {
                    None => {
                        unimplemented!();
                    }
                    Some(data) => {
                        if t.participant.sel.compare_and_swap(0, 1, SeqCst) == 0 {
                            t.participant.thread.unpark();
                        }
                    }
                }
            }
        }

        true
    }

    fn is_closed(&self) -> bool {
        let _lock = self.lock.lock().unwrap();
        self.closed.load(SeqCst)
    }

    fn monitor_tx(&self) -> &Monitor {
        unimplemented!()
    }

    fn monitor_rx(&self) -> &Monitor {
        unimplemented!()
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        if cfg!(debug_assertions) {
            let mut lock = self.lock.get_mut().unwrap();
            debug_assert_eq!(lock.senders.len(), 0);
            debug_assert_eq!(lock.receivers.len(), 0);
            debug_assert_eq!(self.senders_len.load(SeqCst), 0);
            debug_assert_eq!(self.receivers_len.load(SeqCst), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;
    use std::time::Duration;

    use crossbeam;

    use bounded;
    use err::*;

    // TODO: drop test

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn smoke() {
        let (tx, rx) = bounded(0);
        assert_eq!(tx.try_send(7), Err(TrySendError::Full(7)));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn recv() {
        let (tx, rx) = bounded(0);

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
        let (tx, rx) = bounded(0);

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
        let (tx, rx) = bounded(0);

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
        let (tx, rx) = bounded(0);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(tx.send(7), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(tx.send(8), Ok(()));
                thread::sleep(ms(100));
                assert_eq!(tx.send(9), Ok(()));
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
        let (tx, rx) = bounded(0);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(
                    tx.send_timeout(7, ms(100)),
                    Err(SendTimeoutError::Timeout(7))
                );
                assert_eq!(tx.send_timeout(8, ms(100)), Ok(()));
                assert_eq!(
                    tx.send_timeout(9, ms(100)),
                    Err(SendTimeoutError::Disconnected(9))
                );
            });
            s.spawn(move || {
                thread::sleep(ms(150));
                assert_eq!(rx.recv(), Ok(8));
            });
        });
    }

    #[test]
    fn try_send() {
        let (tx, rx) = bounded(0);

        crossbeam::scope(|s| {
            s.spawn(move || {
                assert_eq!(tx.try_send(7), Err(TrySendError::Full(7)));
                thread::sleep(ms(150));
                assert_eq!(tx.try_send(8), Ok(()));
                thread::sleep(ms(50));
                assert_eq!(tx.try_send(9), Err(TrySendError::Disconnected(9)));
            });
            s.spawn(move || {
                thread::sleep(ms(100));
                assert_eq!(rx.recv(), Ok(8));
            });
        });
    }

    #[test]
    fn close_signals_sender() {
        let (tx, rx) = bounded(0);

        crossbeam::scope(|s| {
            s.spawn(move || {
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
        let (tx, rx) = bounded::<()>(0);

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

        let (tx, rx) = bounded(0);

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

        let (tx, rx) = bounded::<usize>(0);
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
