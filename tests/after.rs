//! Tests for the after channel flavor.

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
        use std::time::{Duration, Instant};

        use super::channel::{Select, TryRecvError};
        use $channel as channel;
        use crossbeam;

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        #[test]
        fn fire() {
            let start = Instant::now();
            let r = channel::after(ms(50));

            assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
            thread::sleep(ms(100));

            let fired = r.try_recv().unwrap();
            assert!(start < fired);
            assert!(fired - start >= ms(50));

            let now = Instant::now();
            assert!(fired < now);
            assert!(now - fired >= ms(50));

            assert_eq!(r.try_recv(), Err(TryRecvError::Empty));

            select! {
                recv(r) -> _ => panic!(),
                default => {}
            }

            select! {
                recv(r) -> _ => panic!(),
                recv(channel::after(ms(200))) -> _ => {}
            }
        }

        #[test]
        fn capacity() {
            const COUNT: usize = 10;

            for i in 0..COUNT {
                let r = channel::after(ms(i as u64));
                assert_eq!(r.capacity(), Some(1));
            }
        }

        #[test]
        fn len_empty_full() {
            let r = channel::after(ms(50));

            assert_eq!(r.len(), 0);
            assert_eq!(r.is_empty(), true);
            assert_eq!(r.is_full(), false);

            thread::sleep(ms(100));

            assert_eq!(r.len(), 1);
            assert_eq!(r.is_empty(), false);
            assert_eq!(r.is_full(), true);

            r.try_recv().unwrap();

            assert_eq!(r.len(), 0);
            assert_eq!(r.is_empty(), true);
            assert_eq!(r.is_full(), false);
        }

        #[test]
        fn try_recv() {
            let r = channel::after(ms(200));
            assert!(r.try_recv().is_err());

            thread::sleep(ms(100));
            assert!(r.try_recv().is_err());

            thread::sleep(ms(200));
            assert!(r.try_recv().is_ok());
            assert!(r.try_recv().is_err());

            thread::sleep(ms(200));
            assert!(r.try_recv().is_err());
        }

        #[test]
        fn recv() {
            let start = Instant::now();
            let r = channel::after(ms(50));

            let fired = r.recv().unwrap();
            assert!(start < fired);
            assert!(fired - start >= ms(50));

            let now = Instant::now();
            assert!(fired < now);
            assert!(now - fired < fired - start);

            assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
        }

        #[test]
        fn recv_timeout() {
            let start = Instant::now();
            let r = channel::after(ms(200));

            assert!(r.recv_timeout(ms(100)).is_err());
            let now = Instant::now();
            assert!(now - start >= ms(100));
            assert!(now - start <= ms(150));

            let fired = r.recv_timeout(ms(200)).unwrap();
            assert!(fired - start >= ms(200));
            assert!(fired - start <= ms(250));

            assert!(r.recv_timeout(ms(200)).is_err());
            let now = Instant::now();
            assert!(now - start >= ms(400));
            assert!(now - start <= ms(450));

            assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
        }

        #[test]
        fn recv_two() {
            let r1 = channel::after(ms(50));
            let r2 = channel::after(ms(50));

            crossbeam::scope(|scope| {
                scope.spawn(|| {
                    select! {
                        recv(r1) -> _ => {}
                        recv(r2) -> _ => {}
                    }
                });
                scope.spawn(|| {
                    select! {
                        recv(r1) -> _ => {}
                        recv(r2) -> _ => {}
                    }
                });
            });
        }

        #[test]
        fn recv_race() {
            select! {
                recv(channel::after(ms(50))) -> _ => {}
                recv(channel::after(ms(100))) -> _ => panic!(),
            }

            select! {
                recv(channel::after(ms(100))) -> _ => panic!(),
                recv(channel::after(ms(50))) -> _ => {}
            }
        }

        #[test]
        fn stress_default() {
            const COUNT: usize = 10;

            for _ in 0..COUNT {
                select! {
                    recv(channel::after(ms(0))) -> _ => {}
                    default => panic!(),
                }
            }

            for _ in 0..COUNT {
                select! {
                    recv(channel::after(ms(100))) -> _ => panic!(),
                    default => {}
                }
            }
        }

        #[test]
        fn select_shared() {
            const THREADS: usize = 4;
            const COUNT: usize = 1000;
            const TIMEOUT_MS: u64 = 100;

            let v = (0..COUNT).map(|i| channel::after(ms(i as u64 / TIMEOUT_MS / 2)))
                .collect::<Vec<_>>();
            let hits = AtomicUsize::new(0);

            crossbeam::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        let v: Vec<&_> = v.iter().collect();

                        loop {
                            let timeout = channel::after(ms(TIMEOUT_MS));
                            let mut sel = Select::new();
                            for r in &v {
                                sel.recv(r);
                            }
                            let case_timeout = sel.recv(&timeout);

                            let case = sel.select();
                            match case.index() {
                                i if i == case_timeout => {
                                    case.recv(&timeout).unwrap();
                                    break;
                                }
                                i => {
                                    case.recv(&v[i]).unwrap();
                                    hits.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        }
                    });
                }
            });

            assert_eq!(hits.load(Ordering::SeqCst), COUNT);
        }

        #[test]
        fn select_cloned() {
            const THREADS: usize = 4;
            const COUNT: usize = 1000;
            const TIMEOUT_MS: u64 = 100;

            let v = (0..COUNT).map(|i| channel::after(ms(i as u64 / TIMEOUT_MS / 2)))
                .collect::<Vec<_>>();
            let hits = AtomicUsize::new(0);

            crossbeam::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        loop {
                            let timeout = channel::after(ms(TIMEOUT_MS));
                            let mut sel = Select::new();
                            for r in &v {
                                sel.recv(r);
                            }
                            let case_timeout = sel.recv(&timeout);

                            let case = sel.select();
                            match case.index() {
                                i if i == case_timeout => {
                                    case.recv(&timeout).unwrap();
                                    break;
                                }
                                i => {
                                    case.recv(&v[i]).unwrap();
                                    hits.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        }
                    });
                }
            });

            assert_eq!(hits.load(Ordering::SeqCst), COUNT);
        }

        #[test]
        fn stress_clone() {
            const RUNS: usize = 1000;
            const THREADS: usize = 10;
            const COUNT: usize = 50;

            for i in 0..RUNS {
                let r = channel::after(ms(i as u64));

                crossbeam::scope(|scope| {
                    for _ in 0..THREADS {
                        scope.spawn(|| {
                            let r = r.clone();
                            let _ = r.try_recv();

                            for _ in 0..COUNT {
                                drop(r.clone());
                                thread::yield_now();
                            }
                        });
                    }
                });
            }
        }

        #[test]
        fn fairness() {
            const COUNT: usize = 1000;

            for &dur in &[0, 1] {
                let mut hits = [0usize; 2];

                for _ in 0..COUNT {
                    select! {
                        recv(channel::after(ms(dur))) -> _ => hits[0] += 1,
                        recv(channel::after(ms(dur))) -> _ => hits[1] += 1,
                    }
                }

                assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 2));
            }
        }

        #[test]
        fn fairness_duplicates() {
            const COUNT: usize = 1000;

            for &dur in &[0, 1] {
                let mut hits = [0usize; 5];

                for _ in 0..COUNT {
                    let r = channel::after(ms(dur));
                    select! {
                        recv(r) -> _ => hits[0] += 1,
                        recv(r) -> _ => hits[1] += 1,
                        recv(r) -> _ => hits[2] += 1,
                        recv(r) -> _ => hits[3] += 1,
                        recv(r) -> _ => hits[4] += 1,
                    }
                }

                assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 2));
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

mod select {
    tests!(wrappers::select);
}

mod select_spin {
    tests!(wrappers::select_spin);
}
