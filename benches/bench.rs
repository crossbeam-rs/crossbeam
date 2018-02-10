#![feature(test)]

extern crate crossbeam;
extern crate test;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::time::Duration;

use crossbeam::scope;
use crossbeam::sync::MsQueue;
use crossbeam::sync::SegQueue;

use test::Bencher;

mod mpsc_queue;
use mpsc_queue::Queue as MpscQueue;

const COUNT: u64 = 10000000;
const THREADS: u64 = 2;

fn time<F: FnOnce()>(f: F) -> Duration {
    let start = ::std::time::Instant::now();
    f();
    start.elapsed()
}

fn nanos(d: Duration) -> f64 {
    d.as_secs() as f64 * 1000000000f64 + (d.subsec_nanos() as f64)
}

trait Queue<T> {
    fn push(&self, T);
    fn try_pop(&self) -> Option<T>;
}

impl<T> Queue<T> for MsQueue<T> {
    fn push(&self, t: T) {
        self.push(t)
    }
    fn try_pop(&self) -> Option<T> {
        self.try_pop()
    }
}

impl<T> Queue<T> for SegQueue<T> {
    fn push(&self, t: T) {
        self.push(t)
    }
    fn try_pop(&self) -> Option<T> {
        self.try_pop()
    }
}

impl<T> Queue<T> for MpscQueue<T> {
    fn push(&self, t: T) {
        self.push(t)
    }
    fn try_pop(&self) -> Option<T> {
        use mpsc_queue::*;

        loop {
            match self.pop() {
                Data(t) => return Some(t),
                Empty => return None,
                Inconsistent => (),
            }
        }
    }
}

impl<T> Queue<T> for Mutex<VecDeque<T>> {
    fn push(&self, t: T) {
        self.lock().unwrap().push_back(t)
    }
    fn try_pop(&self) -> Option<T> {
        self.lock().unwrap().pop_front()
    }
}

fn bench_queue_mpsc<Q: Queue<u64> + Sync>(q: Q) -> f64 {
    let d = time(|| {
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
            while count < COUNT * THREADS {
                if q.try_pop().is_some() {
                    count += 1;
                }
            }
        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_queue_mpmc<Q: Queue<bool> + Sync>(q: Q) -> f64 {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::Relaxed;

    let prod_count = AtomicUsize::new(0);

    let d = time(|| {
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
                scope.spawn(move || loop {
                    if let Some(false) = qr.try_pop() {
                        break;
                    }
                });
            }
        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_chan_mpsc() -> f64 {
    let (tx, rx) = channel();

    let d = time(|| {
        scope(|scope| {
            for _i in 0..THREADS {
                let my_tx = tx.clone();

                scope.spawn(move || {
                    for x in 0..COUNT {
                        let _ = my_tx.send(x);
                    }
                });
            }

            for _i in 0..COUNT * THREADS {
                let _ = rx.recv().unwrap();
            }
        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

#[bench]
fn bench_queue_mpsc_ms_queue(b: &mut Bencher) {
    b.iter(|| bench_queue_mpsc(MsQueue::new()));
}

#[bench]
fn bench_queue_mpsc_seg_queue(b: &mut Bencher) {
    b.iter(|| bench_queue_mpsc(SegQueue::new()));
}

#[bench]
fn bench_queue_mpsc_mpsc_queue(b: &mut Bencher) {
    b.iter(|| bench_queue_mpsc(MpscQueue::new()));
}

#[bench]
fn bench_queue_mpsc_chan(b: &mut Bencher) {
    b.iter(|| bench_chan_mpsc());
}

#[bench]
fn bench_queue_mpmc_ms_queue(b: &mut Bencher) {
    b.iter(|| bench_queue_mpmc(MsQueue::new()));
}

#[bench]
fn bench_queue_mpmc_seg_queue(b: &mut Bencher) {
    b.iter(|| bench_queue_mpmc(SegQueue::new()));
}

#[bench]
fn stress_ms_queue(b: &mut Bencher) {
    use std::sync::Arc;

    const DUP: usize = 4;
    const THREADS: u32 = 2;
    const COUNT: u64 = 100000;

    b.iter(|| {
        scope(|s| {
            for _i in 0..DUP {
                let q = Arc::new(MsQueue::new());
                let qs = q.clone();

                s.spawn(move || {
                    for i in 1..COUNT {
                        qs.push(i)
                    }
                });

                for _i in 0..THREADS {
                    let qr = q.clone();
                    s.spawn(move || {
                        let mut cur: u64 = 0;
                        for _j in 0..COUNT {
                            if let Some(new) = qr.try_pop() {
                                assert!(new > cur);
                                cur = new;
                            }
                        }
                    });
                }
            }
        });
    })
}
