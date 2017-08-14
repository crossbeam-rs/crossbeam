extern crate crossbeam;
extern crate multiqueue;

use std::thread;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq(cap: usize) {
    let (tx, rx) = multiqueue::mpmc_queue::<i32>(cap as u64);

    for i in 0..MESSAGES {
        tx.try_send(i as i32).unwrap();
    }
    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc(cap: usize) {
    let (tx, rx) = multiqueue::mpmc_queue::<i32>(cap as u64);

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                while tx.try_send(i as i32).is_err() {
                    thread::yield_now();
                }
            }
        });
        s.spawn(move || {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn mpsc(cap: usize) {
    let (tx, rx) = multiqueue::mpmc_queue::<i32>(cap as u64);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    while tx.try_send(i as i32).is_err() {
                        thread::yield_now();
                    }
                }
            });
        }
        s.spawn(move || {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn mpmc(cap: usize) {
    let (tx, rx) = multiqueue::mpmc_queue::<i32>(cap as u64);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    while tx.try_send(i as i32).is_err() {
                        thread::yield_now();
                    }
                }
            });
        }
        for _ in 0..THREADS {
            let rx = rx.clone();
            s.spawn(move || {
                for _ in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
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
                "Rust multiqueue",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("bounded1_mpmc", mpmc(1));
    run!("bounded1_mpsc", mpsc(1));
    run!("bounded1_spsc", spsc(1));

    // run!("bounded_mpmc", mpmc(MESSAGES));
    run!("bounded_mpsc", mpsc(MESSAGES));
    run!("bounded_seq", seq(MESSAGES));
    run!("bounded_spsc", spsc(MESSAGES));
}
