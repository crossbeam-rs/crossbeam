extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;
extern crate rand;

use std::panic;
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
fn send_panic() {
    let (s, r) = bounded::<i32>(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| s.send(1));
        assert_eq!(r.recv(), Some(1));
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            panic::catch_unwind(|| {
                select! {
                    send(s, panic!()) => {}
                }
            }).err().unwrap();
        });

        thread::sleep(ms(500));
        select! {
            recv(r, _) => panic!(),
            default(ms(500)) => {}
        }
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            panic::catch_unwind(|| {
                select! {
                    send(s, panic!()) => {}
                }
            }).err().unwrap();
        });

        select! {
            recv(r, _) => panic!(),
            default(ms(1000)) => {}
        }
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| s.send(2));
        assert_eq!(r.recv(), Some(2));
    });

    // TODO: mpmc where sending sometimes panics (mpmc_panic)
}

#[test]
fn smoke() {
    let (s, r) = bounded(0);
    select! {
        send(s, 7) => panic!(),
        default => {}
    }
    assert_eq!(r.try_recv(), None);

    assert_eq!(s.capacity(), Some(0));
    assert_eq!(r.capacity(), Some(0));
}

#[test]
fn recv() {
    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.recv(), Some(7));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(8));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(9));
            assert_eq!(r.recv(), None);
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            s.send(7);
            s.send(8);
            s.send(9);
        });
    });
}

#[test]
fn recv_timeout() {
    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            select! {
                recv(r, _) => panic!(),
                default(ms(1000)) => {}
            }
            select! {
                recv(r, v) => assert_eq!(v, Some(7)),
                default(ms(1000)) => panic!(),
            }
            select! {
                recv(r, v) => assert_eq!(v, None),
                default(ms(1000)) => panic!(),
            }
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            s.send(7);
        });
    });
}

#[test]
fn try_recv() {
    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.try_recv(), None);
            thread::sleep(ms(1500));
            assert_eq!(r.try_recv(), Some(7));
            thread::sleep(ms(500));
            assert_eq!(r.try_recv(), None);
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            s.send(7);
        });
    });
}

#[test]
fn send() {
    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            s.send(7);
            thread::sleep(ms(1000));
            s.send(8);
            thread::sleep(ms(1000));
            s.send(9);
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(r.recv(), Some(7));
            assert_eq!(r.recv(), Some(8));
            assert_eq!(r.recv(), Some(9));
        });
    });
}

#[test]
fn send_timeout() {
    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            select! {
                send(s, 7) => panic!(),
                default(ms(1000)) => {}
            }
            select! {
                send(s, 8) => {}
                default(ms(1000)) => panic!(),
            }
            select! {
                send(s, 9) => panic!(),
                default(ms(1000)) => {}
            }
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(r.recv(), Some(8));
        });
    });
}

#[test]
fn try_send() {
    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            select! {
                send(s, 7) => panic!(),
                default => {}
            }
            thread::sleep(ms(1500));
            select! {
                send(s, 8) => {}
                default => panic!(),
            }
            thread::sleep(ms(500));
            select! {
                send(s, 9) => panic!(),
                default => {}
            }
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(8));
        });
    });
}

#[test]
fn len() {
    const COUNT: usize = 25_000;

    let (s, r) = bounded(0);

    assert_eq!(s.len(), 0);
    assert_eq!(r.len(), 0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                assert_eq!(r.recv(), Some(i));
                assert_eq!(r.len(), 0);
            }
        });

        scope.spawn(|| {
            for i in 0..COUNT {
                s.send(i);
                assert_eq!(s.len(), 0);
            }
        });
    });

    assert_eq!(s.len(), 0);
    assert_eq!(r.len(), 0);
}

#[test]
fn close_signals_receiver() {
    let (s, r) = bounded::<()>(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.recv(), None);
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            drop(s);
        });
    });
}

#[test]
fn spsc() {
    const COUNT: usize = 100_000;

    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            for i in 0..COUNT {
                assert_eq!(r.recv(), Some(i));
            }
            assert_eq!(r.recv(), None);
        });
        scope.spawn(move || {
            for i in 0..COUNT {
                s.send(i);
            }
        });
    });
}

#[test]
fn mpmc() {
    const COUNT: usize = 25_000;
    const THREADS: usize = 4;

    let (s, r) = bounded::<usize>(0);
    let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                for _ in 0..COUNT {
                    let n = r.recv().unwrap();
                    v[n].fetch_add(1, SeqCst);
                }
            });
        }
        for _ in 0..THREADS {
            scope.spawn(|| {
                for i in 0..COUNT {
                    s.send(i);
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

    let (s, r) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(50));
                }
                loop {
                    select! {
                        send(s, i) => break,
                        default(ms(10)) => {}
                    }
                }
            }
        });

        scope.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(50));
                }
                loop {
                    select! {
                        recv(r, v) => {
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
        let (s, r) = bounded::<DropCounter>(0);

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..steps {
                    r.recv().unwrap();
                }
            });

            scope.spawn(|| {
                for _ in 0..steps {
                    s.send(DropCounter);
                }
            });
        });

        assert_eq!(DROPS.load(SeqCst), steps);
        drop(s);
        drop(r);
        assert_eq!(DROPS.load(SeqCst), steps);
    }
}
