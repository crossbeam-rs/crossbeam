use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use actor::{self, Actor};
use watch::dock::{Dock, Request, Entry, Packet};

struct Inner {
    closed: bool,
}

pub struct Queue<T> {
    lock: Mutex<Inner>,
    senders: Dock<T>,
    receivers: Dock<T>,
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            lock: Mutex::new(Inner { closed: false }),
            senders: Dock::new(),
            receivers: Dock::new(),
        }
    }

    pub fn promise_send(&self) {
        let _lock = self.lock.lock();
        self.senders.register_promise();
    }

    pub fn unpromise_send(&self) {
        let _lock = self.lock.lock();
        self.senders.unregister();
    }

    pub fn promise_recv(&self) {
        let _lock = self.lock.lock();
        self.receivers.register_promise();
    }

    pub fn unpromise_recv(&self) {
        let _lock = self.lock.lock();
        self.receivers.unregister();
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut lock = self.lock.lock();
        if lock.closed {
            return Err(TrySendError::Disconnected(value));
        }

        // TODO: should ignore the current thread here...
        while let Some(entry) = self.receivers.pop() {
            match entry {
                Entry::Promise { actor } => {
                    if actor.select(self.rx_id()) {
                        let req = Request::new(Some(value));
                        actor::current().reset();
                        actor.set_request(&req);
                        drop(lock);

                        actor.unpark();
                        actor::current().wait_until(None);
                        return Ok(());
                    }
                }
                Entry::Offer { actor, packet } => {
                    if actor.select(self.rx_id()) {
                        unsafe { (*packet).put(value) }
                        actor.unpark();
                        return Ok(());
                    }
                }
            }
        }

        Err(TrySendError::Full(value))
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            match self.try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(v)) => value = v,
                Err(TrySendError::Disconnected(v)) => {
                    return Err(SendTimeoutError::Disconnected(v))
                }
            }

            let packet;
            {
                let mut lock = self.lock.lock();
                if lock.closed {
                    return Err(SendTimeoutError::Disconnected(value));
                }

                actor::current().reset();
                packet = Packet::new(Some(value));
                self.senders.register_offer(&packet);

                if lock.closed || !self.receivers.is_empty() {
                    actor::current().select(1);
                }
            }

            if !actor::current().wait_until(deadline) {
                return Err(SendTimeoutError::Timeout(packet.take()));
            } else if actor::current().selected() != 1 {
                let mut lock = self.lock.lock();
                return Ok(());
            } else {
                self.senders.unregister();
                value = packet.take();
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut lock = self.lock.lock();
        if lock.closed {
            return Err(TryRecvError::Disconnected);
        }

        while let Some(entry) = self.senders.pop() {
            match entry {
                Entry::Promise { actor } => {
                    if actor.select(self.tx_id()) {
                        let req = Request::new(None);
                        actor::current().reset();
                        actor.set_request(&req);
                        drop(lock);

                        actor.unpark();
                        actor::current().wait_until(None);
                        let lock = self.lock.lock();
                        let v = req.packet.take();
                        return Ok(v);
                    }
                }
                Entry::Offer { actor, packet } => {
                    if actor.select(self.tx_id()) {
                        let v = unsafe { (*packet).take() };
                        actor.unpark();
                        return Ok(v);
                    }
                }
            }
        }

        Err(TryRecvError::Empty)
    }

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            }

            let packet;
            {
                let mut lock = self.lock.lock();
                if lock.closed {
                    return Err(RecvTimeoutError::Disconnected);
                }

                actor::current().reset();
                packet = Packet::new(None);
                self.receivers.register_offer(&packet);

                if lock.closed || !self.senders.is_empty() {
                    actor::current().select(1);
                }
            }

            if !actor::current().wait_until(deadline) {
                return Err(RecvTimeoutError::Timeout);
            } else if actor::current().selected() != 1 {
                let mut lock = self.lock.lock();
                return Ok(packet.take());
            } else {
                self.receivers.unregister();
            }
        }
    }

    pub fn has_senders(&self) -> bool {
        let _lock = self.lock.lock();
        !self.senders.is_empty()
    }

    pub fn has_receivers(&self) -> bool {
        let _lock = self.lock.lock();
        !self.receivers.is_empty()
    }

    pub fn close(&self) -> bool {
        let mut lock = self.lock.lock();

        if lock.closed {
            false
        } else {
            lock.closed = true;
            self.senders.notify_all();
            self.receivers.notify_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.lock.lock().closed
    }

    // pub fn is_ready_tx(&self) -> bool {
    //     let lock = self.lock.lock();
    //     if lock.closed {
    //         true
    //     } else {
    //         match self.receivers.0.len() {
    //             0 => false,
    //             1 => !self.receivers.0[0].is_current(),
    //             _ => true,
    //         }
    //     }
    // }
    //
    // pub fn is_ready_rx(&self) -> bool {
    //     let lock = self.lock.lock();
    //     if lock.closed {
    //         true
    //     } else {
    //         match self.senders.0.len() {
    //             0 => false,
    //             1 => !self.senders.0[0].is_current(),
    //             _ => true,
    //         }
    //     }
    // }

    pub fn tx_id(&self) -> usize {
        self as *const _ as usize
    }

    pub fn rx_id(&self) -> usize {
        (self as *const _ as usize) | 1
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        debug_assert!(self.senders.is_empty());
        debug_assert!(self.receivers.is_empty());
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
