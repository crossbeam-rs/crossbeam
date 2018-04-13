extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;
extern crate rand;

use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT};
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;

use crossbeam_channel::bounded;
use rand::{thread_rng, Rng};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke() {
    let (tx, rx) = bounded(0);
    assert_eq!(tx.try_send(7), Some(7));
    assert_eq!(rx.try_recv(), None);

    assert_eq!(tx.capacity(), Some(0));
    assert_eq!(rx.capacity(), Some(0));
}

#[test]
fn recv() {
    let (tx, rx) = bounded(0);

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
    let (tx, rx) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(move || {
            select! {
                recv(rx, v) => panic!(),
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
    let (tx, rx) = bounded(0);

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
    let (tx, rx) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(move || {
            tx.send(7);
            thread::sleep(ms(1000));
            tx.send(8);
            thread::sleep(ms(1000));
            tx.send(9);
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
    let (tx, rx) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(move || {
            select! {
                send(tx, 7) => panic!(),
                default(ms(1000)) => {}
            }
            select! {
                send(tx, 8) => {}
                default(ms(1000)) => panic!(),
            }
            select! {
                send(tx, 9) => panic!(),
                default(ms(1000)) => {}
            }
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(rx.recv(), Some(8));
        });
    });
}

#[test]
fn try_send() {
    let (tx, rx) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(tx.try_send(7), Some(7));
            thread::sleep(ms(1500));
            assert_eq!(tx.try_send(8), None);
            thread::sleep(ms(500));
            assert_eq!(tx.try_send(9), Some(9));
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(8));
        });
    });
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
                assert_eq!(rx.recv(), Some(i));
                assert_eq!(rx.len(), 0);
            }
        });

        s.spawn(|| {
            for i in 0..COUNT {
                tx.send(i);
                assert_eq!(tx.len(), 0);
            }
        });
    });

    assert_eq!(tx.len(), 0);
    assert_eq!(rx.len(), 0);
}

#[test]
fn close_signals_receiver() {
    let (tx, rx) = bounded::<()>(0);

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

    let (tx, rx) = bounded(0);

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

    let (tx, rx) = bounded::<usize>(0);
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

    let (tx, rx) = bounded(0);

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

    #[derive(Debug, PartialEq)]
    struct DropCounter;

    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, SeqCst);
        }
    }

    let mut rng = thread_rng();

    for _ in 0..100 {
        let steps = rng.gen_range(0, 3_000);

        DROPS.store(0, SeqCst);
        let (tx, rx) = bounded::<DropCounter>(0);

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

        assert_eq!(DROPS.load(SeqCst), steps);
        drop(tx);
        drop(rx);
        assert_eq!(DROPS.load(SeqCst), steps);
    }
}
