#[macro_use]
pub extern crate crossbeam_channel as channel;

extern crate crossbeam;
extern crate rand;

mod wrappers;

macro_rules! tests {
    ($channel:path) => {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        use std::thread;
        use std::time::{Duration, Instant};

        use $channel as channel;
        use crossbeam;
        use rand::{thread_rng, Rng};

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        #[test]
        fn fire() {
            let start = Instant::now();
            let r = channel::after(ms(50));

            assert_eq!(r.try_recv(), None);
            thread::sleep(ms(100));

            let fired = r.try_recv().unwrap();
            assert!(start < fired);
            assert!(fired - start >= ms(50));

            let now = Instant::now();
            assert!(fired < now);
            assert!(now - fired >= ms(50));

            assert_eq!(r.try_recv(), None);

            select! {
                recv(r) => panic!(),
                default => {}
            }
        }

        #[test]
        fn capacity() {
            for i in 0..10 {
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
        fn recv() {
            let start = Instant::now();
            let r = channel::after(ms(50));

            let fired = r.recv().unwrap();
            assert!(start < fired);
            assert!(fired - start >= ms(50));

            let now = Instant::now();
            assert!(fired < now);
            assert!(now - fired < fired - start);

            assert_eq!(r.try_recv(), None);
        }

        #[test]
        fn recv_two() {
            let r1 = channel::after(ms(50));
            let r2 = channel::after(ms(50));

            crossbeam::scope(|scope| {
                scope.spawn(|| {
                    select! {
                        recv(r1) => {}
                        recv(r2) => {}
                    }
                });
                scope.spawn(|| {
                    select! {
                        recv(r1) => {}
                        recv(r2) => {}
                    }
                });
            });
        }

        #[test]
        fn recv_race() {
            select! {
                recv(channel::after(ms(50))) => {}
                recv(channel::after(ms(100))) => panic!(),
            }

            select! {
                recv(channel::after(ms(100))) => panic!(),
                recv(channel::after(ms(50))) => {}
            }
        }

        #[test]
        fn stress_default() {
            const COUNT: usize = 10_000;

            for _ in 0..COUNT {
                select! {
                    recv(channel::after(ms(0))) => {}
                    default => panic!(),
                }
            }

            for _ in 0..COUNT {
                select! {
                    recv(channel::after(ms(50))) => panic!(),
                    default => {}
                }
            }
        }

        #[test]
        fn select_shared() {
            const COUNT: usize = 1000;
            const TIMEOUT_MS: u64 = 100;

            let v = (0..COUNT).map(|i| channel::after(ms(i as u64 / TIMEOUT_MS / 2)))
                .collect::<Vec<_>>();
            let hits = AtomicUsize::new(0);

            crossbeam::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        let v: Vec<&_> = v.iter().collect();

                        loop {
                            select! {
                                recv(v.iter().map(|r| &r.0)) => {
                                    hits.fetch_add(1, Ordering::SeqCst);
                                }
                                recv(channel::after(ms(TIMEOUT_MS))) => break
                            }
                        }
                    });
                }
            });

            assert_eq!(hits.load(Ordering::SeqCst), COUNT);
        }

        #[test]
        fn select_cloned() {
            const COUNT: usize = 1000;
            const TIMEOUT_MS: u64 = 100;

            let v = (0..COUNT).map(|i| channel::after(ms(i as u64 / TIMEOUT_MS / 2)))
                .collect::<Vec<_>>();
            let hits = AtomicUsize::new(0);

            crossbeam::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        let v = v.clone();

                        loop {
                            select! {
                                recv(v.iter().map(|r| &r.0)) => {
                                    hits.fetch_add(1, Ordering::SeqCst);
                                }
                                recv(channel::after(ms(TIMEOUT_MS))) => break
                            }
                        }
                    });
                }
            });

            assert_eq!(hits.load(Ordering::SeqCst), COUNT);
        }

        #[test]
        fn stress_clone() {
            for i in 0..1000 {
                let r = channel::after(ms(i));

                crossbeam::scope(|scope| {
                    for _ in 0..10 {
                        scope.spawn(|| {
                            let r = r.clone();
                            r.try_recv();

                            for _ in 0..50 {
                                r.clone();
                                thread::yield_now();
                            }
                        });
                    }
                });
            }
        }

        #[test]
        fn fairness() {
            const COUNT: usize = 10_000;

            let mut hits = [0usize; 2];
            for _ in 0..COUNT {
                select! {
                    recv(channel::after(ms(0))) => hits[0] += 1,
                    recv(channel::after(ms(0))) => hits[1] += 1,
                }
            }
            assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 2));
        }

        #[test]
        fn fairness_duplicates() {
            const COUNT: usize = 10_000;

            let mut hits = [0usize; 5];
            for _ in 0..COUNT {
                let r = channel::after(ms(0));
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

mod multiselect {
    tests!(wrappers::multiselect);
}
