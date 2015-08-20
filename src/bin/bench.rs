#![feature(duration_span)]

extern crate crossbeam;

use std::sync::mpsc::channel;
use std::time::Duration;

use crossbeam::thread::scope;
use crossbeam::queue::Queue;

const COUNT: u64 = 1000000;
const THREADS: u64 = 3;

fn bench_chan_mpsc() -> f64 {
    let (tx, rx) = channel();

    let d = Duration::span(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let my_tx = tx.clone();

                scope.spawn(move || {
                    for x in 0..COUNT {
                        let _ = my_tx.send(x);
                    }
                });
            }

            for _i in 0..COUNT*THREADS {
                let _ = rx.recv().unwrap();
            }
        });
    });

    d.subsec_nanos() as f64 / ((COUNT * THREADS) as f64)
}

fn bench_queue_mpsc() -> f64 {
    let q = Queue::new();

    let d = Duration::span(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let qr = &q;
                scope.spawn(move || {
                    for x in 0..COUNT {
                        let _ = qr.push(x);
                    }
                });
            }

            let mut count = 0;
            while count < COUNT*THREADS {
                if q.pop().is_some() {
                    count += 1;
                }
            }
        });
    });

    d.subsec_nanos() as f64 / ((COUNT * THREADS) as f64)
}

fn bench_alloc() -> f64 {
    let d = Duration::span(|| {
        for i in 0..COUNT*THREADS {
            Box::new(i);
        }
    });
    d.subsec_nanos() as f64 / ((COUNT * THREADS) as f64)
}

fn main() {
    println!("chan_mpsc: {}", bench_chan_mpsc());
    println!("queue_mpsc: {}", bench_queue_mpsc());
    //println!("alloc: {}", bench_alloc());
}
