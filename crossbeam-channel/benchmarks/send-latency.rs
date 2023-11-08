use crossbeam::channel::unbounded;

mod message;

const MESSAGES: usize = 10_000_000;
const THREADS: usize = 4;

fn clock() -> f64 {
    extern "C" {
        pub fn clock() -> libc::clock_t;
    }
    unsafe { clock() as f64 }
}

struct Stats {
    min: f64,
    max: f64,
    mean: f64,
    _s: f64,
}

impl Stats {
    fn new() -> Self {
        Self {
            min: f64::MAX,
            max: f64::MIN,
            mean: 0.0,
            _s: 0.0,
        }
    }

    fn update(&mut self, x: f64, i: usize) {
        self.min = self.min.min(x);
        self.max = self.max.max(x);
        let mean_prev = self.mean;
        self.mean += (x - self.mean) / ((i + 1) as f64);
        self._s = self._s + ((x - mean_prev) * (x - self.mean));
    }

    fn stddev(&self, i: usize) -> f64 {
        (self._s / (i as f64)).sqrt()
    }
}

fn spsc() -> (f64, f64, f64, f64) {
    let (tx, rx) = unbounded();
    let mut stats = Stats::new();
    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES / 1000 {
                let before = clock();
                for i in 0..1000 {
                    tx.send(message::new(i)).unwrap();
                }
                let elapsed = clock() - before;
                stats.update(elapsed, i)
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    })
    .unwrap();
    (stats.min, stats.max, stats.mean, stats.stddev(MESSAGES - 1))
}

fn mpsc() -> (f64, f64, f64, f64) {
    let (tx, rx) = unbounded();
    let mut stats = Stats::new();
    crossbeam::scope(|scope| {
        for _ in 0..THREADS - 1 {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message::new(i)).unwrap();
                }
            });
        }
        scope.spawn(|_| {
            for i in 0..MESSAGES / THREADS / 1000 {
                let before = clock();
                for i in 0..1000 {
                    tx.send(message::new(i)).unwrap();
                }
                let elapsed = clock() - before;
                stats.update(elapsed, i)
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    })
    .unwrap();
    (stats.min, stats.max, stats.mean, stats.stddev(MESSAGES - 1))
}

fn spmc() -> (f64, f64, f64, f64) {
    let (tx, rx) = unbounded();
    let mut stats = Stats::new();
    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES / 1000 {
                let before = clock();
                for i in 0..1000 {
                    tx.send(message::new(i)).unwrap();
                }
                let elapsed = clock() - before;
                stats.update(elapsed, i)
            }
        });

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
                }
            });
        }
    })
    .unwrap();
    (stats.min, stats.max, stats.mean, stats.stddev(MESSAGES - 1))
}

fn mpmc() -> (f64, f64, f64, f64) {
    let (tx, rx) = unbounded();
    let mut stats = Stats::new();
    crossbeam::scope(|scope| {
        for _ in 0..THREADS - 1 {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message::new(i)).unwrap();
                }
            });
        }
        scope.spawn(|_| {
            for i in 0..MESSAGES / THREADS / 1000 {
                let before = clock();
                for i in 0..1000 {
                    tx.send(message::new(i)).unwrap();
                }
                let elapsed = clock() - before;
                stats.update(elapsed, i)
            }
        });

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
                }
            });
        }
    })
    .unwrap();
    (stats.min, stats.max, stats.mean, stats.stddev(MESSAGES - 1))
}

fn run(name: &str, (min, max, mean, stddev): (f64, f64, f64, f64)) {
    println!(
        "{} | min: {:10.3}, max: {:10.3}, mean: {:10.3}, stddev: {:10.3}",
        name, min, max, mean, stddev
    );
}

fn main() {
    run("spsc", spsc());
    run("spsc", spsc());
    run("mpsc", mpsc());
    run("mpsc", mpsc());
    run("spmc", spmc());
    run("spmc", spmc());
    run("mpmc", mpmc());
    run("mpmc", mpmc());
}
