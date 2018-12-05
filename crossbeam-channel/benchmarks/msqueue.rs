extern crate crossbeam;

use crossbeam::queue::MsQueue;
use std::thread;

mod message;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq() {
    let q = MsQueue::new();

    for i in 0..MESSAGES {
        q.push(message::new(i));
    }

    for _ in 0..MESSAGES {
        q.try_pop().unwrap();
    }
}

fn spsc() {
    let q = MsQueue::new();

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES {
                q.push(message::new(i));
            }
        });

        for _ in 0..MESSAGES {
            loop {
                if q.try_pop().is_none() {
                    thread::yield_now();
                } else {
                    break;
                }
            }
        }
    }).unwrap();
}

fn mpsc() {
    let q = MsQueue::new();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS {
                    q.push(message::new(i));
                }
            });
        }

        for _ in 0..MESSAGES {
            loop {
                if q.try_pop().is_none() {
                    thread::yield_now();
                } else {
                    break;
                }
            }
        }
    }).unwrap();
}

fn mpmc() {
    let q = MsQueue::new();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS {
                    q.push(message::new(i));
                }
            });
        }

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..MESSAGES / THREADS {
                    loop {
                        if q.try_pop().is_none() {
                            thread::yield_now();
                        } else {
                            break;
                        }
                    }
                }
            });
        }
    }).unwrap();
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
                "Rust msqueue",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    run!("unbounded_mpmc", mpmc());
    run!("unbounded_mpsc", mpsc());
    run!("unbounded_seq", seq());
    run!("unbounded_spsc", spsc());
}
