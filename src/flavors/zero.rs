use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use parking_lot::Mutex;

use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use actor::{self, Actor, HandleId, Packet, Request};

struct Inner<T> {
    senders: Registry<T>,
    receivers: Registry<T>,
    closed: bool,
}

pub struct Channel<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Channel<T> {
    pub fn new() -> Self {
        Channel {
            inner: Mutex::new(Inner {
                senders: Registry::new(),
                receivers: Registry::new(),
                closed: false,
            }),
        }
    }

    pub fn promise_send(&self, id: HandleId) {
        self.inner.lock().senders.register_promise(id);
    }

    pub fn revoke_send(&self, id: HandleId) {
        self.inner.lock().senders.unregister(id);
    }

    pub fn promise_recv(&self, id: HandleId) {
        self.inner.lock().receivers.register_promise(id);
    }

    pub fn revoke_recv(&self, id: HandleId) {
        self.inner.lock().receivers.unregister(id);
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(TrySendError::Disconnected(value));
        }

        while let Some(e) = inner.receivers.pop() {
            match e.packet {
                None => {
                    if e.actor.select(e.id) {
                        let req = Request::new(Some(value));
                        actor::current().reset();
                        e.actor.set_request(&req);
                        drop(inner);

                        e.actor.unpark();
                        actor::current().wait_until(None);
                        return Ok(());
                    }
                }
                Some(packet) => {
                    if e.actor.select(e.id) {
                        unsafe { (*packet).put(value) }
                        e.actor.unpark();
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
                Err(TrySendError::Disconnected(v)) => return Err(SendTimeoutError::Disconnected(v)),
            }

            let packet;
            {
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(SendTimeoutError::Disconnected(value));
                }

                if inner.receivers.can_notify() {
                    continue;
                }

                actor::current().reset();
                packet = Packet::new(Some(value));
                inner.senders.register_offer(&packet, HandleId::sentinel());
            }

            let timed_out = !actor::current().wait_until(deadline);
            let mut inner = self.inner.lock();
            inner.senders.unregister(HandleId::sentinel());

            match packet.take() {
                None => return Ok(()),
                Some(v) => value = v,
            }

            if timed_out {
                return Err(SendTimeoutError::Timeout(value));
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(TryRecvError::Disconnected);
        }

        while let Some(e) = inner.senders.pop() {
            match e.packet {
                None => {
                    if e.actor.select(e.id) {
                        let req = Request::new(None);
                        actor::current().reset();
                        e.actor.set_request(&req);
                        drop(inner);

                        e.actor.unpark();
                        actor::current().wait_until(None);
                        let _inner = self.inner.lock();
                        let v = req.packet.take().unwrap();
                        return Ok(v);
                    }
                }
                Some(packet) => {
                    if e.actor.select(e.id) {
                        let v = unsafe { (*packet).take().unwrap() };
                        e.actor.unpark();
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
                let mut inner = self.inner.lock();
                if inner.closed {
                    return Err(RecvTimeoutError::Disconnected);
                }

                if inner.senders.can_notify() {
                    continue;
                }

                actor::current().reset();
                packet = Packet::new(None);
                inner
                    .receivers
                    .register_offer(&packet, HandleId::sentinel());
            }

            let timed_out = !actor::current().wait_until(deadline);
            let mut inner = self.inner.lock();
            inner.receivers.unregister(HandleId::sentinel());

            if let Some(v) = packet.take() {
                return Ok(v);
            }

            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    pub fn can_recv(&self) -> bool {
        self.inner.lock().senders.can_notify()
    }

    pub fn can_send(&self) -> bool {
        self.inner.lock().receivers.can_notify()
    }

    pub fn close(&self) -> bool {
        let mut inner = self.inner.lock();

        if inner.closed {
            false
        } else {
            inner.closed = true;
            inner.senders.notify_all();
            inner.receivers.notify_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }
}

pub struct Entry<T> {
    actor: Arc<Actor>,
    id: HandleId,
    packet: Option<*const Packet<T>>,
}

struct Registry<T> {
    entries: VecDeque<Entry<T>>,
}

impl<T> Registry<T> {
    fn new() -> Self {
        Registry {
            entries: VecDeque::new(),
        }
    }

    fn pop(&mut self) -> Option<Entry<T>> {
        let thread_id = thread::current().id();

        for i in 0..self.entries.len() {
            if self.entries[i].actor.thread_id() != thread_id {
                let e = self.entries.remove(i).unwrap();
                return Some(e);
            }
        }

        None
    }

    fn register_offer(&mut self, packet: *const Packet<T>, id: HandleId) {
        self.entries.push_back(Entry {
            actor: actor::current(),
            id,
            packet: Some(packet),
        });
    }

    fn register_promise(&mut self, id: HandleId) {
        self.entries.push_back(Entry {
            actor: actor::current(),
            id,
            packet: None,
        });
    }

    fn unregister(&mut self, id: HandleId) {
        let thread_id = thread::current().id();
        self.entries
            .retain(|e| e.actor.thread_id() != thread_id || e.id != id);
        self.maybe_shrink();
    }

    fn can_notify(&self) -> bool {
        let thread_id = thread::current().id();

        for i in 0..self.entries.len() {
            if self.entries[i].actor.thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    fn notify_all(&mut self) {
        for e in self.entries.drain(..) {
            e.actor.select(e.id);
            e.actor.unpark();
        }
        self.maybe_shrink();
    }

    fn maybe_shrink(&mut self) {
        if self.entries.capacity() > 32 && self.entries.capacity() / 2 > self.entries.len() {
            self.entries.shrink_to_fit();
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
