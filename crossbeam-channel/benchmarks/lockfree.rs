use lockfree::channel;

mod message;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

use std::thread;

fn seq() {
    let (mut tx, mut rx) = channel::spsc::create();

    for i in 0..MESSAGES {
        tx.send(message::new(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        while rx.recv().is_err() {
            thread::yield_now();
        }
    }
}

fn spsc() {
    let (mut tx, mut rx) = channel::spsc::create();

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES {
                tx.send(message::new(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            while rx.recv().is_err() {
                thread::yield_now();
            }
        }
    });
}

fn mpsc() {
    let (tx, mut rx) = channel::mpsc::create();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message::new(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            while rx.recv().is_err() {
                thread::yield_now();
            }
        }
    });
}

fn mpmc() {
    let (tx, rx) = channel::mpmc::create();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message::new(i)).unwrap();
                }
            });
        }

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..MESSAGES / THREADS {
                    while rx.recv().is_err() {
                        thread::yield_now();
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
                "Rust crossbeam-channel",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    run!("unbounded_mpmc", mpmc());
    run!("unbounded_mpsc", mpsc());
    run!("unbounded_seq", seq());
    run!("unbounded_spsc", spsc());
}
