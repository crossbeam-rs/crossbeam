//! Tests for the tick channel flavor.

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

        use $channel as channel;
        use crossbeam;

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        #[test]
        fn fire() {
            let start = Instant::now();
            let r = channel::tick(ms(100));

            assert_eq!(r.try_recv(), None);
            thread::sleep(ms(200));

            let fired = r.try_recv().unwrap();
            assert!(start < fired);
            assert!(fired - start >= ms(100));

            let now = Instant::now();
            assert!(fired < now);
            assert!(now - fired >= ms(100));

            assert_eq!(r.try_recv(), None);

            select! {
                recv(r) => panic!(),
                default => {}
            }

            select! {
                recv(r) => {}
                recv(channel::after(ms(400))) => panic!(),
            }
        }

        #[test]
        fn intervals() {
            let start = Instant::now();
            let r = channel::tick(ms(200));

            let t1 = r.recv().unwrap();
            assert!(start + ms(200) <= t1);
            assert!(start + ms(400) > t1);

            thread::sleep(ms(1200));
            let t2 = r.try_recv().unwrap();
            assert!(start + ms(400) <= t2);
            assert!(start + ms(600) > t2);

            assert_eq!(r.try_recv(), None);
            let t3 = r.recv().unwrap();
            assert!(start + ms(1600) <= t3);
            assert!(start + ms(1800) > t3);

            assert_eq!(r.try_recv(), None);
        }

        #[test]
        fn capacity() {
            const COUNT: usize = 10;

            for i in 0..COUNT {
                let r = channel::tick(ms(i as u64));
                assert_eq!(r.capacity(), Some(1));
            }
        }

        #[test]
        fn len_empty_full() {
            let r = channel::tick(ms(100));

            assert_eq!(r.len(), 0);
            assert_eq!(r.is_empty(), true);
            assert_eq!(r.is_full(), false);

            thread::sleep(ms(200));

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
            let r = channel::tick(ms(100));

            let fired = r.recv().unwrap();
            assert!(start < fired);
            assert!(fired - start >= ms(100));

            let now = Instant::now();
            assert!(fired < now);
            assert!(now - fired < fired - start);

            assert_eq!(r.try_recv(), None);
        }

        #[test]
        fn recv_two() {
            let r1 = channel::tick(ms(100));
            let r2 = channel::tick(ms(100));

            crossbeam::scope(|scope| {
                scope.spawn(|| {
                    for _ in 0..10 {
                        select! {
                            recv(r1) => {}
                            recv(r2) => {}
                        }
                    }
                });
                scope.spawn(|| {
                    for _ in 0..10 {
                        select! {
                            recv(r1) => {}
                            recv(r2) => {}
                        }
                    }
                });
            });
        }

        #[test]
        fn recv_race() {
            select! {
                recv(channel::tick(ms(100))) => {}
                recv(channel::tick(ms(200))) => panic!(),
            }

            select! {
                recv(channel::tick(ms(200))) => panic!(),
                recv(channel::tick(ms(100))) => {}
            }
        }

        #[test]
        fn stress_default() {
            const COUNT: usize = 10;

            for _ in 0..COUNT {
                select! {
                    recv(channel::tick(ms(0))) => {}
                    default => panic!(),
                }
            }

            for _ in 0..COUNT {
                select! {
                    recv(channel::tick(ms(100))) => panic!(),
                    default => {}
                }
            }
        }

        #[test]
        fn select() {
            const THREADS: usize = 4;

            let hits = AtomicUsize::new(0);
            let r1 = channel::tick(ms(400));
            let r2 = channel::tick(ms(600));

            crossbeam::scope(|scope| {
                for _ in 0..THREADS {
                    scope.spawn(|| {
                        let v = vec![&r1.0, &r2.0];
                        let timeout = channel::after(ms(2200));

                        loop {
                            select! {
                                recv(v) => {
                                    hits.fetch_add(1, Ordering::SeqCst);
                                }
                                recv(timeout) => break
                            }
                        }
                    });
                }
            });

            assert_eq!(hits.load(Ordering::SeqCst), 8);
        }

        #[test]
        fn fairness() {
            const COUNT: usize = 30;

            for &dur in &[0, 1] {
                let mut hits = [0usize; 2];

                for _ in 0..COUNT {
                    let r1 = channel::tick(ms(dur));
                    let r2 = channel::tick(ms(dur));

                    for _ in 0..COUNT {
                        select! {
                            recv(r1) => hits[0] += 1,
                            recv(r2) => hits[1] += 1,
                        }
                    }
                }

                assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 2));
            }
        }

        #[test]
        fn fairness_duplicates() {
            const COUNT: usize = 30;

            for &dur in &[0, 1] {
                let mut hits = [0usize; 5];

                for _ in 0..COUNT {
                    let r = channel::tick(ms(dur));

                    for _ in 0..COUNT {
                        select! {
                            recv(r) => hits[0] += 1,
                            recv(r) => hits[1] += 1,
                            recv(r) => hits[2] += 1,
                            recv(r) => hits[3] += 1,
                            recv(r) => hits[4] += 1,
                        }
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

mod select_multi {
    tests!(wrappers::select_multi);
}
