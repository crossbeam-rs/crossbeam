#![feature(duration_span)]

extern crate crossbeam;

use std::sync::mpsc::channel;
use std::time::Duration;

use crossbeam::scope;
use crossbeam::sync::MsQueue;

const COUNT: u64 = 1000000;
const THREADS: u64 = 2;

fn nanos(d: Duration) -> f64 {
    d.as_secs() as f64 * 1000000000f64 + (d.subsec_nanos() as f64)
}

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

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_queue_mpsc() -> f64 {
    let q = MsQueue::new();

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

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_queue_mpmc() -> f64 {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    let q = MsQueue::new();
    let prod_count = AtomicUsize::new(0);

    let d = Duration::span(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let qr = &q;
                let pcr = &prod_count;
                scope.spawn(move || {
                    for _x in 0..COUNT {
                        qr.push(true);
                    }
                    if pcr.fetch_add(1, Relaxed) == (THREADS as usize) - 1 {
                        for _x in 0..THREADS {
                            qr.push(false)
                        }
                    }
                });
                scope.spawn(move || {
                    loop {
                        if let Some(false) = qr.pop() { break }
                    }
                });
            }


        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_mutex_mpmc() -> f64 {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    let q = Mutex::new(VecDeque::new());
    let prod_count = AtomicUsize::new(0);

    let d = Duration::span(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let qr = &q;
                let pcr = &prod_count;
                scope.spawn(move || {
                    for _x in 0..COUNT {
                        qr.lock().unwrap().push_back(true);
                    }
                    if pcr.fetch_add(1, Relaxed) == (THREADS as usize) - 1 {
                        for _x in 0..THREADS {
                            qr.lock().unwrap().push_back(false);
                        }
                    }
                });
                scope.spawn(move || {
                    loop {
                        if let Some(false) = qr.lock().unwrap().pop_front() { break }
                    }
                });
            }


        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn main() {
    println!("chan_mpsc: {}", bench_chan_mpsc());
    println!("queue_mpsc: {}", bench_queue_mpsc());
    println!("queue_mpmc: {}", bench_queue_mpmc());
    println!("mutex_mpmc: {}", bench_mutex_mpmc());
}
