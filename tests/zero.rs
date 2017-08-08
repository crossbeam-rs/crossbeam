extern crate channel;
extern crate crossbeam;

use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use channel::select;
use channel::{RecvError, SendError, RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};

// TODO: drop test

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke() {
    let (tx, rx) = channel::bounded(0);
    assert_eq!(tx.try_send(7), Err(TrySendError::Full(7)));
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn recv() {
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded(0);

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
    let (tx, rx) = channel::bounded::<()>(0);

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

    let (tx, rx) = channel::bounded(0);

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

    let (tx, rx) = channel::bounded::<usize>(0);
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
