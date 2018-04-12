extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;
extern crate rand;

use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT};

use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use crossbeam_channel::bounded;
use crossbeam_channel::{RecvError, TryRecvError};
use crossbeam_channel::TrySendError;
use rand::{thread_rng, Rng};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke() {
    let (tx, rx) = bounded(1);
    tx.try_send(7).unwrap();
    assert_eq!(rx.try_recv(), Some(7));

    tx.send(8);
    assert_eq!(rx.recv(), Some(8));

    assert_eq!(rx.try_recv(), None);
    select! {
        recv(rx, _) => panic!(),
        default(ms(1000)) => {}
    }
}

#[test]
fn capacity() {
    for i in 1..10 {
        let (tx, rx) = bounded::<()>(i);
        assert_eq!(tx.capacity(), Some(i));
        assert_eq!(rx.capacity(), Some(i));
    }
}

#[test]
fn recv() {
    let (tx, rx) = bounded(100);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.recv(), Some(7));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(8));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(9));
            assert_eq!(rx.recv(), None);
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            tx.send(7);
            tx.send(8);
            tx.send(9);
        });
    });
}

#[test]
fn recv_timeout() {
    let (tx, rx) = bounded(100);

    crossbeam::scope(|s| {
        s.spawn(move || {
            select! {
                recv(rx, _) => panic!(),
                default(ms(1000)) => {}
            }
            select! {
                recv(rx, v) => assert_eq!(v, Some(7)),
                default(ms(1000)) => panic!(),
            }
            select! {
                recv(rx, v) => assert_eq!(v, None),
                default(ms(1000)) => panic!(),
            }
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            tx.send(7);
        });
    });
}

#[test]
fn try_recv() {
    let (tx, rx) = bounded(100);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.try_recv(), None);
            thread::sleep(ms(1500));
            assert_eq!(rx.try_recv(), Some(7));
            thread::sleep(ms(500));
            assert_eq!(rx.try_recv(), None);
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            tx.send(7);
        });
    });
}

#[test]
fn send() {
    let (tx, rx) = bounded(1);

    crossbeam::scope(|s| {
        s.spawn(move || {
            tx.send(7);
            thread::sleep(ms(1000));
            tx.send(8);
            thread::sleep(ms(1000));
            tx.send(9);
            thread::sleep(ms(1000));
            assert_eq!(tx.try_send(10), Ok(()));
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(rx.recv(), Some(7));
            assert_eq!(rx.recv(), Some(8));
            assert_eq!(rx.recv(), Some(9));
        });
    });
}

#[test]
fn send_timeout() {
    let (tx, rx) = bounded(2);

    crossbeam::scope(|s| {
        s.spawn(move || {
            select! {
                send(tx, 1) => {}
                default(ms(1000)) => panic!(),
            }
            select! {
                send(tx, 2) => {}
                default(ms(1000)) => panic!(),
            }
            select! {
                send(tx, 3) => panic!(),
                default(ms(500)) => {}
            }
            thread::sleep(ms(1000));
            select! {
                send(tx, 4) => {}
                default(ms(1000)) => panic!(),
            }
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(1));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(2));
            assert_eq!(rx.recv(), Some(4));
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
            thread::sleep(ms(1500));
            assert_eq!(tx.try_send(3), Ok(()));
            thread::sleep(ms(500));
            assert_eq!(tx.try_send(4), Ok(()));
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(rx.try_recv(), Some(1));
            assert_eq!(rx.try_recv(), None);
            assert_eq!(rx.recv(), Some(3));
        });
    });
}

#[test]
fn recv_after_close() {
    let (tx, rx) = bounded(100);

    tx.send(1);
    tx.send(2);
    tx.send(3);

    drop(tx);

    assert_eq!(rx.recv(), Some(1));
    assert_eq!(rx.recv(), Some(2));
    assert_eq!(rx.recv(), Some(3));
    assert_eq!(rx.recv(), None);
}

#[test]
fn len() {
    const COUNT: usize = 25_000;
    const CAP: usize = 1000;

    let (tx, rx) = bounded(CAP);

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);

    for _ in 0..CAP / 10 {
        for i in 0..50 {
            tx.send(i);
            assert_eq!(tx.len(), i + 1);
        }

        for i in 0..50 {
            rx.recv().unwrap();
            assert_eq!(rx.len(), 50 - i - 1);
        }
    }

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);

    for i in 0..CAP {
        tx.send(i);
        assert_eq!(tx.len(), i + 1);
    }

    for _ in 0..CAP {
        rx.recv().unwrap();
    }

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..COUNT {
                assert_eq!(rx.recv(), Some(i));
                let len = rx.len();
                assert!(len <= CAP);
            }
        });

        s.spawn(|| {
            for i in 0..COUNT {
                tx.send(i);
                let len = tx.len();
                assert!(len <= CAP);
            }
        });
    });

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);
}

#[test]
fn close_signals_receiver() {
    let (tx, rx) = bounded::<()>(1);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.recv(), None);
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
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
                assert_eq!(rx.recv(), Some(i));
            }
            assert_eq!(rx.recv(), None);
        });
        s.spawn(move || {
            for i in 0..COUNT {
                tx.send(i);
            }
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
            s.spawn(|| {
                for _ in 0..COUNT {
                    let n = rx.recv().unwrap();
                    v[n].fetch_add(1, SeqCst);
                }
            });
        }
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..COUNT {
                    tx.send(i);
                }
            });
        }
    });

    for c in v {
        assert_eq!(c.load(SeqCst), THREADS);
    }
}

#[test]
fn stress_timeout_two_threads() {
    const COUNT: usize = 100;

    let (tx, rx) = bounded(2);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(50));
                }
                loop {
                    select! {
                        send(tx, i) => break,
                        default(ms(10)) => {}
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
                    select! {
                        recv(rx, v) => {
                            assert_eq!(v, Some(i));
                            break;
                        }
                        default(ms(10)) => {}
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
        let additional = rng.gen_range(0, 50);

        DROPS.store(0, SeqCst);
        let (tx, rx) = bounded::<DropCounter>(50);

        crossbeam::scope(|s| {
            s.spawn(|| {
                for _ in 0..steps {
                    rx.recv().unwrap();
                }
            });

            s.spawn(|| {
                for _ in 0..steps {
                    tx.send(DropCounter);
                }
            });
        });

        for _ in 0..additional {
            tx.try_send(DropCounter).unwrap();
        }

        assert_eq!(DROPS.load(SeqCst), steps);
        drop(tx);
        drop(rx);
        assert_eq!(DROPS.load(SeqCst), steps + additional);
    }
}

#[test]
fn linearizable() {
    const COUNT: usize = 25_000;
    const THREADS: usize = 4;

    let (tx, rx) = bounded(THREADS);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..COUNT {
                    tx.send(0);
                    rx.try_recv().unwrap();
                }
            });
        }
    });
}
