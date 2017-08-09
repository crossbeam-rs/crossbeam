extern crate channel;
extern crate crossbeam;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::{Duration, Instant};

use channel::select;
use channel::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

// TODO: drop test
// TODO: len test

#[test]
fn smoke() {
    let (tx, rx) = channel::unbounded();
    tx.try_send(7).unwrap();
    assert_eq!(rx.try_recv().unwrap(), 7);

    tx.send(8);
    assert_eq!(rx.recv().unwrap(), 8);

    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(rx.recv_timeout(ms(100)), Err(RecvTimeoutError::Timeout));
}

#[test]
fn recv() {
    let (tx, rx) = channel::unbounded();

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
    let (tx, rx) = channel::unbounded();

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
    let (tx, rx) = channel::unbounded();

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
fn recv_after_close() {
    let (tx, rx) = channel::unbounded();

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
fn close_signals_receiver() {
    let (tx, rx) = channel::unbounded::<()>();

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

    let (tx, rx) = channel::unbounded();

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

    let (tx, rx) = channel::unbounded::<usize>();
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
