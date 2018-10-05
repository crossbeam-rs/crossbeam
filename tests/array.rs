//! Tests for the array channel flavor.

extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;
extern crate rand;

mod wrappers;

macro_rules! tests {
    ($channel:path) => {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        use std::thread;
        use std::time::Duration;

        use super::channel::{
            RecvError, SendError, RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError
        };
        use $channel as channel;
        use crossbeam;
        use rand::{thread_rng, Rng};

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        #[test]
        fn smoke() {
            let (s, r) = channel::bounded(1);
            s.send(7).unwrap();
            assert_eq!(r.try_recv(), Ok(7));

            s.send(8).unwrap();
            assert_eq!(r.recv(), Ok(8));

            assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
            assert_eq!(r.recv_timeout(ms(1000)), Err(RecvTimeoutError::Timeout));
        }

        #[test]
        fn capacity() {
            for i in 1..10 {
                let (s, r) = channel::bounded::<()>(i);
                assert_eq!(s.capacity(), Some(i));
                assert_eq!(r.capacity(), Some(i));
            }
        }

        #[test]
        fn len_empty_full() {
            let (s, r) = channel::bounded(2);

            assert_eq!(s.len(), 0);
            assert_eq!(s.is_empty(), true);
            assert_eq!(s.is_full(), false);
            assert_eq!(r.len(), 0);
            assert_eq!(r.is_empty(), true);
            assert_eq!(r.is_full(), false);

            s.send(()).unwrap();

            assert_eq!(s.len(), 1);
            assert_eq!(s.is_empty(), false);
            assert_eq!(s.is_full(), false);
            assert_eq!(r.len(), 1);
            assert_eq!(r.is_empty(), false);
            assert_eq!(r.is_full(), false);

            s.send(()).unwrap();

            assert_eq!(s.len(), 2);
            assert_eq!(s.is_empty(), false);
            assert_eq!(s.is_full(), true);
            assert_eq!(r.len(), 2);
            assert_eq!(r.is_empty(), false);
            assert_eq!(r.is_full(), true);

            r.recv().unwrap();

            assert_eq!(s.len(), 1);
            assert_eq!(s.is_empty(), false);
            assert_eq!(s.is_full(), false);
            assert_eq!(r.len(), 1);
            assert_eq!(r.is_empty(), false);
            assert_eq!(r.is_full(), false);
        }

        #[test]
        fn recv() {
            let (s, r) = channel::bounded(100);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(r.recv(), Ok(7));
                    thread::sleep(ms(1000));
                    assert_eq!(r.recv(), Ok(8));
                    thread::sleep(ms(1000));
                    assert_eq!(r.recv(), Ok(9));
                    assert_eq!(r.recv(), Err(RecvError));
                });
                scope.spawn(move || {
                    thread::sleep(ms(1500));
                    s.send(7).unwrap();
                    s.send(8).unwrap();
                    s.send(9).unwrap();
                });
            });
        }

        #[test]
        fn recv_timeout() {
            let (s, r) = channel::bounded::<i32>(100);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(r.recv_timeout(ms(1000)), Err(RecvTimeoutError::Timeout));
                    assert_eq!(r.recv_timeout(ms(1000)), Ok(7));
                    assert_eq!(
                        r.recv_timeout(ms(1000)),
                        Err(RecvTimeoutError::Disconnected)
                    );
                });
                scope.spawn(move || {
                    thread::sleep(ms(1500));
                    s.send(7).unwrap();
                });
            });
        }

        #[test]
        fn try_recv() {
            let (s, r) = channel::bounded(100);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
                    thread::sleep(ms(1500));
                    assert_eq!(r.try_recv(), Ok(7));
                    thread::sleep(ms(500));
                    assert_eq!(r.try_recv(), Err(TryRecvError::Disconnected));
                });
                scope.spawn(move || {
                    thread::sleep(ms(1000));
                    s.send(7).unwrap();
                });
            });
        }

        #[test]
        fn send() {
            let (s, r) = channel::bounded(1);

            crossbeam::scope(|scope| {
                scope.spawn(|| {
                    s.send(7).unwrap();
                    thread::sleep(ms(1000));
                    s.send(8).unwrap();
                    thread::sleep(ms(1000));
                    s.send(9).unwrap();
                    thread::sleep(ms(1000));
                    s.send(10).unwrap();
                });
                scope.spawn(|| {
                    thread::sleep(ms(1500));
                    assert_eq!(r.recv(), Ok(7));
                    assert_eq!(r.recv(), Ok(8));
                    assert_eq!(r.recv(), Ok(9));
                });
            });
        }

        #[test]
        fn send_timeout() {
            let (s, r) = channel::bounded(2);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(s.send_timeout(1, ms(1000)), Ok(()));
                    assert_eq!(s.send_timeout(2, ms(1000)), Ok(()));
                    assert_eq!(
                        s.send_timeout(3, ms(500)),
                        Err(SendTimeoutError::Timeout(3))
                    );
                    thread::sleep(ms(1000));
                    assert_eq!(s.send_timeout(4, ms(1000)), Ok(()));
                    thread::sleep(ms(1000));
                    assert_eq!(s.send(5), Err(SendError(5)));
                });
                scope.spawn(move || {
                    thread::sleep(ms(1000));
                    assert_eq!(r.recv(), Ok(1));
                    thread::sleep(ms(1000));
                    assert_eq!(r.recv(), Ok(2));
                    assert_eq!(r.recv(), Ok(4));
                });
            });
        }

        #[test]
        fn try_send() {
            let (s, r) = channel::bounded(1);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(s.try_send(1), Ok(()));
                    assert_eq!(s.try_send(2), Err(TrySendError::Full(2)));
                    thread::sleep(ms(1500));
                    assert_eq!(s.try_send(3), Ok(()));
                    thread::sleep(ms(500));
                    assert_eq!(s.try_send(4), Err(TrySendError::Disconnected(4)));
                });
                scope.spawn(move || {
                    thread::sleep(ms(1000));
                    assert_eq!(r.try_recv(), Ok(1));
                    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
                    assert_eq!(r.recv(), Ok(3));
                });
            });
        }

        #[test]
        fn recv_after_close() {
            let (s, r) = channel::bounded(100);

            s.send(1).unwrap();
            s.send(2).unwrap();
            s.send(3).unwrap();

            drop(s);

            assert_eq!(r.recv(), Ok(1));
            assert_eq!(r.recv(), Ok(2));
            assert_eq!(r.recv(), Ok(3));
            assert_eq!(r.recv(), Err(RecvError));
        }

        #[test]
        fn len() {
            const COUNT: usize = 25_000;
            const CAP: usize = 1000;

            let (s, r) = channel::bounded(CAP);

            assert_eq!(s.len(), 0);
            assert_eq!(r.len(), 0);

            for _ in 0..CAP / 10 {
                for i in 0..50 {
                    s.send(i).unwrap();
                    assert_eq!(s.len(), i + 1);
                }

                for i in 0..50 {
                    r.recv().unwrap();
                    assert_eq!(r.len(), 50 - i - 1);
                }
            }

            assert_eq!(s.len(), 0);
            assert_eq!(r.len(), 0);

            for i in 0..CAP {
                s.send(i).unwrap();
                assert_eq!(s.len(), i + 1);
            }

            for _ in 0..CAP {
                r.recv().unwrap();
            }

            assert_eq!(s.len(), 0);
            assert_eq!(r.len(), 0);

            crossbeam::scope(|scope| {
                scope.spawn(|| {
                    for i in 0..COUNT {
                        assert_eq!(r.recv(), Ok(i));
                        let len = r.len();
                        assert!(len <= CAP);
                    }
                });

                scope.spawn(|| {
                    for i in 0..COUNT {
                        s.send(i).unwrap();
                        let len = s.len();
                        assert!(len <= CAP);
                    }
                });
            });

            assert_eq!(s.len(), 0);
            assert_eq!(r.len(), 0);
        }

        #[test]
        fn close_wakes_sender() {
            let (s, r) = channel::bounded(1);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(s.send(()), Ok(()));
                    assert_eq!(s.send(()), Err(SendError(())));
                });
                scope.spawn(move || {
                    thread::sleep(ms(1000));
                    drop(r);
                });
            });
        }

        #[test]
        fn close_wakes_receiver() {
            let (s, r) = channel::bounded::<()>(1);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    assert_eq!(r.recv(), Err(RecvError));
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

            let (s, r) = channel::bounded(3);

            crossbeam::scope(|scope| {
                scope.spawn(move || {
                    for i in 0..COUNT {
                        assert_eq!(r.recv(), Ok(i));
                    }
                    assert_eq!(r.recv(), Err(RecvError));
                });
                scope.spawn(move || {
                    for i in 0..COUNT {
                        s.send(i).unwrap();
                    }
                });
            });
        }

        #[test]
        fn mpmc() {
            const COUNT: usize = 25_000;
            const THREADS: usize = 4;

            let (s, r) = channel::bounded::<usize>(3);
            let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

            crossbeam::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        for _ in 0..COUNT {
                            let n = r.recv().unwrap();
                            v[n].fetch_add(1, Ordering::SeqCst);
                        }
                    });
                }
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        for i in 0..COUNT {
                            s.send(i).unwrap();
                        }
                    });
                }
            });

            for c in v {
                assert_eq!(c.load(Ordering::SeqCst), THREADS);
            }
        }

        #[test]
        fn stress_timeout_two_threads() {
            const COUNT: usize = 100;

            let (s, r) = channel::bounded(2);

            crossbeam::scope(|scope| {
                scope.spawn(|| {
                    for i in 0..COUNT {
                        if i % 2 == 0 {
                            thread::sleep(ms(50));
                        }
                        loop {
                            if let Ok(()) = s.send_timeout(i, ms(10)) {
                                break;
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
                            if let Ok(x) = r.recv_timeout(ms(10)) {
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
            const RUNS: usize = 100;

            static DROPS: AtomicUsize = AtomicUsize::new(0);

            #[derive(Debug, PartialEq)]
            struct DropCounter;

            impl Drop for DropCounter {
                fn drop(&mut self) {
                    DROPS.fetch_add(1, Ordering::SeqCst);
                }
            }

            let mut rng = thread_rng();

            for _ in 0..RUNS {
                let steps = rng.gen_range(0, 10_000);
                let additional = rng.gen_range(0, 50);

                DROPS.store(0, Ordering::SeqCst);
                let (s, r) = channel::bounded::<DropCounter>(50);

                crossbeam::scope(|scope| {
                    scope.spawn(|| {
                        for _ in 0..steps {
                            r.recv().unwrap();
                        }
                    });

                    scope.spawn(|| {
                        for _ in 0..steps {
                            s.send(DropCounter).unwrap();
                        }
                    });
                });

                for _ in 0..additional {
                    s.send(DropCounter).unwrap();
                }

                assert_eq!(DROPS.load(Ordering::SeqCst), steps);
                drop(s);
                drop(r);
                assert_eq!(DROPS.load(Ordering::SeqCst), steps + additional);
            }
        }

        #[test]
        fn linearizable() {
            const COUNT: usize = 25_000;
            const THREADS: usize = 4;

            let (s, r) = channel::bounded(THREADS);

            crossbeam::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        for _ in 0..COUNT {
                            s.send(0).unwrap();
                            r.try_recv().unwrap();
                        }
                    });
                }
            });
        }

        #[test]
        fn fairness() {
            const COUNT: usize = 10_000;

            let (s1, r1) = channel::bounded::<()>(COUNT);
            let (s2, r2) = channel::bounded::<()>(COUNT);

            for _ in 0..COUNT {
                s1.send(()).unwrap();
                s2.send(()).unwrap();
            }

            let mut hits = [0usize; 2];
            for _ in 0..COUNT {
                select! {
                    recv(r1) => hits[0] += 1,
                    recv(r2) => hits[1] += 1,
                }
            }
            assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 2));
        }

        #[test]
        fn fairness_duplicates() {
            const COUNT: usize = 10_000;

            let (s, r) = channel::bounded::<()>(COUNT);

            for _ in 0..COUNT {
                s.send(()).unwrap();
            }

            let mut hits = [0usize; 5];
            for _ in 0..COUNT {
                select! {
                    recv(r) => hits[0] += 1,
                    recv(r) => hits[1] += 1,
                    recv(r) => hits[2] += 1,
                    recv(r) => hits[3] += 1,
                    recv(r) => hits[4] += 1,
                }
            }
            assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 2));
        }

        #[test]
        fn recv_in_send() {
            let (s, _r) = channel::bounded(1);
            s.send(()).unwrap();

            select! {
                send(s, panic!()) => panic!(),
                default => {}
            }

            let (s, r) = channel::bounded(2);
            s.send(()).unwrap();

            select! {
                send(s, assert_eq!(r.recv(), Ok(()))) => {}
            }
        }
    };
}

mod normal {
    tests!(wrappers::normal);
}

mod cloned {
    tests!(wrappers::cloned);
}

// TODO
// mod select {
//     tests!(wrappers::select);
// }
//
// mod select_spin {
//     tests!(wrappers::select_spin);
// }
//
// mod select_multi {
//     tests!(wrappers::select_multi);
// }
