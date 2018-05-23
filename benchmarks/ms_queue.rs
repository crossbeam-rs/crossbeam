extern crate crossbeam;

use crossbeam::sync::MsQueue;
use std::thread;
use testtype::TestType;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

pub mod testtype;

fn seq() {
    let q = MsQueue::<TestType>::new();

    for i in 0..MESSAGES {
        q.push(TestType::new(i));
    }
    for _ in 0..MESSAGES {
        q.try_pop().unwrap();
    }
}

fn spsc() {
    let q = MsQueue::<TestType>::new();

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..MESSAGES {
                q.push(TestType::new(i));
            }
        });
        s.spawn(|| {
            for _ in 0..MESSAGES {
                loop {
                    if q.try_pop().is_none() {
                        if cfg!(feature = "yield") {
                            thread::yield_now();
                        }
                    } else {
                        break;
                    }
                }
            }
        });
    });
}

fn mpsc() {
    let q = MsQueue::<TestType>::new();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    q.push(TestType::new(i));
                }
            });
        }
        s.spawn(|| {
            for _ in 0..MESSAGES {
                loop {
                    if q.try_pop().is_none() {
                        if cfg!(feature = "yield") {
                            thread::yield_now();
                        }
                    } else {
                        break;
                    }
                }
            }
        });
    });
}

fn mpmc() {
    let q = MsQueue::<TestType>::new();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    q.push(TestType::new(i));
                }
            });
        }
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..MESSAGES / THREADS {
                    loop {
                        if q.try_pop().is_none() {
                            if cfg!(feature = "yield") {
                                thread::yield_now();
                            }
                        } else {
                            break;
                        }
                    }
                }
            });
        }
    });
}

fn main() {
    macro_rules! run {
        ($name:expr, $f:expr) => {
            let now = ::std::time::Instant::now();
            $f;
            let elapsed = now.elapsed();
            println!(
                "{:25} {:15} {:7.3} sec",
                $name,
                "Rust MsQueue",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("unbounded_mpmc", mpmc());
    run!("unbounded_mpsc", mpsc());
    run!("unbounded_seq", seq());
    run!("unbounded_spsc", spsc());
}
