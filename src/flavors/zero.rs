use std::cell::UnsafeCell;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Instant;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use actor::{self, Actor};
use watch::dock::{Dock, Request, Entry, Packet};

struct Inner<T> {
    senders: Dock<T>,
    receivers: Dock<T>,
    closed: bool,
}

pub struct Queue<T> {
    lock: Mutex<Inner<T>>,
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            lock: Mutex::new(Inner {
                senders: Dock::new(),
                receivers: Dock::new(),
                closed: false,
            }),
        }
    }

    pub fn promise_recv(&self) {
        self.lock.lock().unwrap().receivers.register_promise();
    }

    pub fn unpromise_recv(&self) {
        self.lock.lock().unwrap().receivers.unregister();
    }

    fn meet_receiver<'a>(
        &'a self,
        value: T,
        mut lock: MutexGuard<'a, Inner<T>>,
    ) -> Result<MutexGuard<'a, Inner<T>>, (T, MutexGuard<'a, Inner<T>>)> {
        loop {
            match lock.receivers.pop() {
                None => return Err((value, lock)),
                Some(Entry::Promise { actor: a }) => {
                    if a.select(self.id()) {
                        let req = Request::new(Some(value));
                        actor::reset();
                        a.set_request(&req);
                        drop(lock);

                        actor::wait();
                        return Ok(self.lock.lock().unwrap());
                    }
                }
                Some(Entry::Offer { actor: a, packet }) => {
                    if a.select(self.id()) {
                        unsafe { (*packet).put(value); }
                        a.unpark();
                        return Ok(lock);
                    }
                }
            }
        }
    }

    fn meet_sender<'a>(
        &'a self,
        mut lock: MutexGuard<'a, Inner<T>>,
    ) -> Result<(T, MutexGuard<'a, Inner<T>>), MutexGuard<'a, Inner<T>>> {
        loop {
            match lock.senders.pop() {
                None => return Err(lock),
                Some(Entry::Promise { actor: a }) => {
                    if a.select(self.id()) {
                        let req = Request::new(None);
                        actor::reset();
                        a.set_request(&req);
                        drop(lock);

                        actor::wait();
                        let lock = self.lock.lock().unwrap();
                        let v = req.packet.take();
                        return Ok((v, lock));
                    }
                }
                Some(Entry::Offer { actor: a, packet }) => {
                    if a.select(self.id()) {
                        let v = unsafe { (*packet).take() };
                        a.unpark();
                        return Ok((v, lock));
                    }
                }
            }
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut lock = self.lock.lock().unwrap();

        if lock.closed {
            Err(TrySendError::Disconnected(value))
        } else {
            match self.meet_receiver(value, lock) {
                Ok(_) => Ok(()),
                Err((v, _)) => Err(TrySendError::Full(v)),
            }
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        let mut lock = self.lock.lock().unwrap();
        if lock.closed {
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
        let packet = Packet::new(Some(value));
        lock.senders.register_offer(&packet);
        drop(lock);

        let timed_out = !actor::wait_until(deadline);
        let mut lock = self.lock.lock().unwrap();
        lock.senders.unregister();

        if timed_out {
            Err(SendTimeoutError::Timeout(packet.take()))
        } else if actor::selected() == 1 {
            Err(SendTimeoutError::Disconnected(packet.take()))
        } else {
            Ok(())
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut lock = self.lock.lock().unwrap();

        if lock.closed {
            Err(TryRecvError::Disconnected)
        } else {
            match self.meet_sender(lock) {
                Ok((v, _)) => Ok(v),
                Err(_) => Err(TryRecvError::Empty),
            }
        }
    }

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let mut lock = self.lock.lock().unwrap();
        if lock.closed {
            return Err(RecvTimeoutError::Disconnected);
        }

        match self.meet_sender(lock) {
            Ok((v, _)) => return Ok(v),
            Err(l) => lock = l,
        }

        actor::reset();
        let packet = Packet::new(None);
        lock.receivers.register_offer(&packet);
        drop(lock);

        let timed_out = !actor::wait_until(deadline);
        let mut lock = self.lock.lock().unwrap();
        lock.receivers.unregister();

        if timed_out {
            Err(RecvTimeoutError::Timeout)
        } else if actor::selected() == 1 {
            Err(RecvTimeoutError::Disconnected)
        } else {
            Ok(packet.take())
        }
    }

    pub fn has_senders(&self) -> bool {
        !self.lock.lock().unwrap().senders.is_empty()
    }

    pub fn has_receivers(&self) -> bool {
        !self.lock.lock().unwrap().receivers.is_empty()
    }

    pub fn close(&self) -> bool {
        let mut lock = self.lock.lock().unwrap();

        if lock.closed {
            false
        } else {
            lock.closed = true;
            lock.senders.notify_all();
            lock.receivers.notify_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.lock.lock().unwrap().closed
    }

    pub fn id(&self) -> usize {
        self as *const _ as usize
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        debug_assert!(self.lock.get_mut().unwrap().senders.is_empty());
        debug_assert!(self.lock.get_mut().unwrap().receivers.is_empty());
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
