use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};
use std::time::Instant;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use impls::Channel;
use actor::{self, ACTOR, Actor, Request};

struct Blocked<T> {
    actor: Arc<Actor>,
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
            actor: actor::current(),
            data: None,
        });
        self.receivers_len.store(lock.receivers.len(), SeqCst);
    }

    pub fn unpromise_recv(&self) {
        let mut lock = self.lock.lock().unwrap();
        ACTOR.with(|a| {
            lock.receivers.retain(|s| !ptr::eq(&*s.actor, &**a));
        });
        self.receivers_len.store(lock.receivers.len(), SeqCst);
    }

    fn meet_receiver<'a>(
        &'a self,
        value: T,
        mut lock: MutexGuard<'a, Inner<T>>,
    ) -> Result<MutexGuard<'a, Inner<T>>, (T, MutexGuard<'a, Inner<T>>)> {
        while let Some(f) = lock.receivers.pop_front() {
            self.receivers_len.store(lock.receivers.len(), SeqCst);
            unsafe {
                match f.data {
                    None => {
                        if f.actor.select_id.compare_and_swap(0, self.id(), SeqCst) == 0 {
                            let req = Request::new(Some(value));
                            actor::reset();
                            f.actor.request_ptr.store(&req as *const _ as usize, SeqCst);
                            drop(lock);

                            actor::wait();
                            return Ok(self.lock.lock().unwrap());
                        }
                    }
                    Some(data) => {
                        if f.actor.select(self.id()) {
                            *(*data).get().as_mut().unwrap() = Some(value);
                            f.actor.unpark();
                            return Ok(lock);
                        }
                    }
                }
            }
        }
        Err((value, lock))
    }

    fn meet_sender<'a>(
        &'a self,
        mut lock: MutexGuard<'a, Inner<T>>,
    ) -> Result<(T, MutexGuard<'a, Inner<T>>), MutexGuard<'a, Inner<T>>> {
        while let Some(f) = lock.senders.pop_front() {
            self.senders_len.store(lock.senders.len(), SeqCst);
            unsafe {
                match f.data {
                    None => {
                        if f.actor.select_id.compare_and_swap(0, self.id(), SeqCst) == 0 {
                            let req = Request::new(None);
                            actor::reset();
                            f.actor.request_ptr.store(&req as *const _ as usize, SeqCst);
                            drop(lock);

                            actor::wait();
                            let lock = self.lock.lock().unwrap();
                            let v = req.data.get().as_mut().unwrap().take().unwrap();
                            return Ok((v, lock));
                        }
                    }
                    Some(data) => {
                        if f.actor.select(self.id()) {
                            let v = (*data).get().as_mut().unwrap().take().unwrap();
                            f.actor.unpark();
                            return Ok((v, lock));
                        }
                    }
                }
            }
        }
        Err(lock)
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

        match self.meet_receiver(value, lock) {
            Ok(_) => Ok(()),
            Err((v, _)) => Err(TrySendError::Full(v)),
        }
    }

    fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        if self.closed.load(SeqCst) {
            return Err(SendTimeoutError::Disconnected(value));
        }

        let cell;
        {
            let mut lock = self.lock.lock().unwrap();

            if self.closed.load(SeqCst) {
                return Err(SendTimeoutError::Disconnected(value));
            }

            match self.meet_receiver(value, lock) {
                Ok(_) => return Ok(()),
                Err((v, l)) => {
                    value = v;
                    lock = l;
                }
            }

            actor::reset();
            cell = UnsafeCell::new(Some(value));
            lock.senders.push_back(Blocked {
                actor: actor::current(),
                data: Some(&cell),
            });
            self.senders_len.store(lock.senders.len(), SeqCst);
        }

        if !actor::wait_until(deadline) {
            let mut lock = self.lock.lock().unwrap();

            lock.senders
                .retain(|s| s.data.map_or(true, |x| !ptr::eq(x, &cell)));
            self.senders_len.store(lock.senders.len(), SeqCst);

            let v = unsafe { cell.get().as_mut().unwrap().take().unwrap() };
            return Err(SendTimeoutError::Timeout(v));
        }

        if ACTOR.with(|a| a.select_id.load(SeqCst)) == 1 {
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

        match self.meet_sender(lock) {
            Ok((v, _)) => Ok(v),
            Err(_) => Err(TryRecvError::Empty),
        }
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

            match self.meet_sender(lock) {
                Ok((v, _)) => return Ok(v),
                Err(l) => lock = l,
            }

            actor::reset();
            cell = UnsafeCell::new(None);
            lock.receivers.push_back(Blocked {
                actor: actor::current(),
                data: Some(&cell),
            });
            self.receivers_len.store(lock.receivers.len(), SeqCst);
        }

        if !actor::wait_until(deadline) {
            let mut lock = self.lock.lock().unwrap();

            lock.receivers
                .retain(|s| s.data.map_or(true, |x| !ptr::eq(x, &cell)));
            self.receivers_len.store(lock.receivers.len(), SeqCst);

            return Err(RecvTimeoutError::Timeout);
        }

        if ACTOR.with(|a| a.select_id.load(SeqCst)) == 1 {
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
                        if t.actor.select_id.compare_and_swap(0, 1, SeqCst) == 0 {
                            t.actor.thread.unpark();
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
                        if t.actor.select_id.compare_and_swap(0, 1, SeqCst) == 0 {
                            t.actor.thread.unpark();
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
