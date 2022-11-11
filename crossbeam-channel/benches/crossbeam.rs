#![feature(test)]

extern crate test;

use crossbeam_channel::{bounded, unbounded};
use crossbeam_utils::thread::scope;
use test::Bencher;

#[derive(Default)]
struct Msg([u64; 4]);

struct Config {
    steps: usize,
    threads: usize,
    spin: usize,
}

impl Config {
    fn from_env() -> Self {
        let steps = std::env::var("STEPS")
            .unwrap_or("40000".to_string())
            .parse()
            .unwrap();
        let threads = std::env::var("THREADS")
            .unwrap_or(std::thread::available_parallelism().unwrap().to_string())
            .parse()
            .unwrap();
        let spin = std::env::var("SPIN")
            .unwrap_or("0".to_string())
            .parse()
            .unwrap();
        Self {
            steps,
            threads,
            spin,
        }
    }

    #[inline(always)]
    fn consume_msg(&self, msg: Msg) {
        test::black_box(msg);
        for i in 0..self.spin {
            test::black_box(i);
        }
    }

    #[inline(always)]
    fn produce_msg(&self) -> Msg {
        for i in 0..self.spin {
            test::black_box(i);
        }
        Msg::default()
    }
}

mod unbounded {
    use super::*;

    #[bench]
    fn create(b: &mut Bencher) {
        b.iter(unbounded::<Msg>);
    }

    #[bench]
    fn oneshot(b: &mut Bencher) {
        let config = Config::from_env();
        b.iter(|| {
            let (s, r) = unbounded::<Msg>();
            s.send(config.produce_msg()).unwrap();
            r.recv().map(|m| config.consume_msg(m)).unwrap();
        });
    }

    #[bench]
    fn inout(b: &mut Bencher) {
        let config = Config::from_env();
        let (s, r) = unbounded::<Msg>();
        b.iter(|| {
            s.send(config.produce_msg()).unwrap();
            r.recv().map(|m| config.consume_msg(m)).unwrap();
        });
    }

    #[bench]
    fn par_inout(b: &mut Bencher) {
        let config = Config::from_env();
        let threads = config.threads;
        let steps = config.steps / config.threads;
        let (s, r) = unbounded::<Msg>();

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..threads {
                    s1.send(config.produce_msg()).unwrap();
                }
                for _ in 0..threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn spsc(b: &mut Bencher) {
        let config = Config::from_env();
        let steps = config.steps;
        let (s, r) = unbounded::<Msg>();

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            scope.spawn(|_| {
                while r1.recv().is_ok() {
                    for _ in 0..steps {
                        s.send(config.produce_msg()).unwrap();
                    }
                    s2.send(()).unwrap();
                }
            });

            b.iter(|| {
                s1.send(()).unwrap();
                for _ in 0..steps {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                r2.recv().unwrap();
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn spmc(b: &mut Bencher) {
        let config = Config::from_env();
        let consum_threads = config.threads - 1;
        let steps = config.steps / consum_threads;
        let (s, r) = unbounded::<Msg>();

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..consum_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..consum_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * consum_threads {
                    s.send(config.produce_msg()).unwrap();
                }
                for _ in 0..consum_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpsc(b: &mut Bencher) {
        let config = Config::from_env();
        let prod_threads = config.threads - 1;
        let steps = config.steps / prod_threads;
        let (s, r) = unbounded::<Msg>();

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..prod_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..prod_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * prod_threads {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                for _ in 0..prod_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpmc(b: &mut Bencher) {
        let config = Config::from_env();
        let threads = config.threads;
        let steps = config.steps / config.threads;
        let (s, r) = unbounded::<Msg>();

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }
}

mod bounded_n {
    use super::*;

    #[bench]
    fn spsc(b: &mut Bencher) {
        let config = Config::from_env();
        let steps = config.steps;
        let (s, r) = bounded::<Msg>(steps);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            scope.spawn(|_| {
                while r1.recv().is_ok() {
                    for _ in 0..steps {
                        s.send(config.produce_msg()).unwrap();
                    }
                    s2.send(()).unwrap();
                }
            });

            b.iter(|| {
                s1.send(()).unwrap();
                for _ in 0..steps {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                r2.recv().unwrap();
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn spmc(b: &mut Bencher) {
        let config = Config::from_env();
        let consum_threads = config.threads - 1;
        let steps = config.steps / consum_threads;
        let (s, r) = bounded::<Msg>(steps * consum_threads);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..consum_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..consum_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * consum_threads {
                    s.send(config.produce_msg()).unwrap();
                }
                for _ in 0..consum_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpsc(b: &mut Bencher) {
        let config = Config::from_env();
        let prod_threads = config.threads - 1;
        let steps = config.steps / prod_threads;
        let (s, r) = bounded::<Msg>(steps * prod_threads);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..prod_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..prod_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * prod_threads {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                for _ in 0..prod_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn par_inout(b: &mut Bencher) {
        let config = Config::from_env();
        let threads = config.threads;
        let steps = config.steps / config.threads;
        let (s, r) = bounded::<Msg>(threads);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpmc(b: &mut Bencher) {
        let config = Config::from_env();
        let threads = config.threads;
        let steps = config.steps / config.threads;
        let (s, r) = bounded::<Msg>(steps * threads);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }
}

mod bounded_1 {
    use super::*;

    #[bench]
    fn create(b: &mut Bencher) {
        b.iter(|| bounded::<Msg>(1));
    }

    #[bench]
    fn oneshot(b: &mut Bencher) {
        let config = Config::from_env();
        b.iter(|| {
            let (s, r) = bounded::<Msg>(1);
            s.send(config.produce_msg()).unwrap();
            r.recv().map(|m| config.consume_msg(m)).unwrap();
        });
    }

    #[bench]
    fn spsc(b: &mut Bencher) {
        let config = Config::from_env();
        let steps = config.steps;
        let (s, r) = bounded::<Msg>(1);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            scope.spawn(|_| {
                while r1.recv().is_ok() {
                    for _ in 0..steps {
                        s.send(config.produce_msg()).unwrap();
                    }
                    s2.send(()).unwrap();
                }
            });

            b.iter(|| {
                s1.send(()).unwrap();
                for _ in 0..steps {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                r2.recv().unwrap();
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn spmc(b: &mut Bencher) {
        let config = Config::from_env();
        let consum_threads = config.threads - 1;
        let steps = config.steps / consum_threads;
        let (s, r) = bounded::<Msg>(1);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..consum_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..consum_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * consum_threads {
                    s.send(config.produce_msg()).unwrap();
                }
                for _ in 0..consum_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpsc(b: &mut Bencher) {
        let config = Config::from_env();
        let prod_threads = config.threads - 1;
        let steps = config.steps / prod_threads;
        let (s, r) = bounded::<Msg>(1);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..prod_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..prod_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * prod_threads {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                for _ in 0..prod_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpmc(b: &mut Bencher) {
        let config = Config::from_env();
        let threads = config.threads;
        let steps = config.steps / config.threads;
        let (s, r) = bounded::<Msg>(1);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }
}

mod bounded_0 {
    use super::*;

    #[bench]
    fn create(b: &mut Bencher) {
        b.iter(|| bounded::<Msg>(0));
    }

    #[bench]
    fn spsc(b: &mut Bencher) {
        let config = Config::from_env();
        let steps = config.steps;
        let (s, r) = bounded::<Msg>(0);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            scope.spawn(|_| {
                while r1.recv().is_ok() {
                    for _ in 0..steps {
                        s.send(config.produce_msg()).unwrap();
                    }
                    s2.send(()).unwrap();
                }
            });

            b.iter(|| {
                s1.send(()).unwrap();
                for _ in 0..steps {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                r2.recv().unwrap();
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn spmc(b: &mut Bencher) {
        let config = Config::from_env();
        let consum_threads = config.threads - 1;
        let steps = config.steps / consum_threads;
        let (s, r) = bounded::<Msg>(0);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..consum_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..consum_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * consum_threads {
                    s.send(config.produce_msg()).unwrap();
                }
                for _ in 0..consum_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpsc(b: &mut Bencher) {
        let config = Config::from_env();
        let prod_threads = config.threads - 1;
        let steps = config.steps / prod_threads;
        let (s, r) = bounded::<Msg>(0);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..prod_threads {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..prod_threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..steps * prod_threads {
                    r.recv().map(|m| config.consume_msg(m)).unwrap();
                }
                for _ in 0..prod_threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }

    #[bench]
    fn mpmc(b: &mut Bencher) {
        let config = Config::from_env();
        let threads = config.threads;
        let steps = config.steps / config.threads;
        let (s, r) = bounded::<Msg>(0);

        let (s1, r1) = bounded(0);
        let (s2, r2) = bounded(0);
        scope(|scope| {
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            s.send(config.produce_msg()).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }
            for _ in 0..threads / 2 {
                scope.spawn(|_| {
                    while r1.recv().is_ok() {
                        for _ in 0..steps {
                            r.recv().map(|m| config.consume_msg(m)).unwrap();
                        }
                        s2.send(()).unwrap();
                    }
                });
            }

            b.iter(|| {
                for _ in 0..threads {
                    s1.send(()).unwrap();
                }
                for _ in 0..threads {
                    r2.recv().unwrap();
                }
            });
            drop(s1);
        })
        .unwrap();
    }
}
