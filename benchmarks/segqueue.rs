extern crate crossbeam;

use std::thread;

use crossbeam::sync::SegQueue;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq() {
    let q = SegQueue::<i32>::new();

    for i in 0..MESSAGES {
        q.push(i as i32);
    }

    for _ in 0..MESSAGES {
        q.try_pop().unwrap();
    }
}

fn spsc() {
    let q = SegQueue::<i32>::new();

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..MESSAGES {
                q.push(i as i32);
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
    });
}

fn mpsc() {
    let q = SegQueue::<i32>::new();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    q.push(i as i32);
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
    });
}

fn mpmc() {
    let q = SegQueue::<i32>::new();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    q.push(i as i32);
                }
            });
        }

        for _ in 0..THREADS {
            s.spawn(|| {
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
                "Rust segqueue",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("unbounded_mpmc", mpmc());
    run!("unbounded_mpsc", mpsc());
    run!("unbounded_seq", seq());
    run!("unbounded_spsc", spsc());
}
