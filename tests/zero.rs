extern crate channel;
extern crate crossbeam;
extern crate rand;

use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT};
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use channel::bounded;
use channel::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use rand::{thread_rng, Rng};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke() {
    let (tx, rx) = bounded(0);
    assert_eq!(tx.try_send(7), Err(TrySendError::Full(7)));
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));

    assert_eq!(tx.capacity(), Some(0));
    assert_eq!(rx.capacity(), Some(0));
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
fn is_disconnected() {
    let (tx, rx) = bounded::<()>(0);
    assert!(!tx.is_disconnected());
    assert!(!rx.is_disconnected());

    let tx2 = tx.clone();
    drop(tx);
    let tx3 = tx2.clone();
    assert!(!tx2.is_disconnected());
    assert!(!rx.is_disconnected());

    drop(tx2);
    assert!(!tx3.is_disconnected());
    assert!(!rx.is_disconnected());

    drop(tx3);
    assert!(rx.is_disconnected());

    let (tx, rx) = bounded::<()>(0);
    assert!(!tx.is_disconnected());
    assert!(!rx.is_disconnected());

    let rx2 = rx.clone();
    drop(rx);
    let rx3 = rx2.clone();
    assert!(!rx2.is_disconnected());
    assert!(!tx.is_disconnected());

    drop(rx2);
    assert!(!rx3.is_disconnected());
    assert!(!tx.is_disconnected());

    drop(rx3);
    assert!(tx.is_disconnected());
}

#[test]
fn len() {
    const COUNT: usize = 25_000;

    let (tx, rx) = bounded(0);

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..COUNT {
                assert_eq!(rx.recv(), Ok(i));
                assert_eq!(rx.len(), 0);
            }
        });

        s.spawn(|| for i in 0..COUNT {
            tx.send(i).unwrap();
            assert_eq!(tx.len(), 0);
        });
    });

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);
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

#[test]
fn stress_timeout_two_threads() {
    const COUNT: usize = 20;

    let (tx, rx) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(50));
                }
                loop {
                    if let Ok(()) = tx.send_timeout(i, ms(10)) {
                        break;
                    }
                }
            }
        });

        s.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(50));
                }
                loop {
                    if let Ok(x) = rx.recv_timeout(ms(10)) {
                        assert_eq!(x, i);
                        break;
                    }
                }
            }
        });
    });
}

#[test]
fn drops() {
    static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;

    struct DropCounter;

    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, SeqCst);
        }
    }

    let mut rng = thread_rng();

    for _ in 0..100 {
        let steps = rng.gen_range(0, 10_000);

        DROPS.store(0, SeqCst);
        let (tx, rx) = bounded::<DropCounter>(0);

        crossbeam::scope(|s| {
            s.spawn(|| for _ in 0..steps {
                rx.recv().unwrap();
            });

            s.spawn(|| for _ in 0..steps {
                tx.send(DropCounter).unwrap();
            });
        });

        assert_eq!(DROPS.load(SeqCst), steps);
        drop(tx);
        drop(rx);
        assert_eq!(DROPS.load(SeqCst), steps);
    }
}
