extern crate crossbeam;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::mpsc::channel;
use std::time::Duration;

use crossbeam::scope;
use crossbeam::sync::MsQueue;
use crossbeam::sync::SegQueue;
use crossbeam::sync::{SpscBufferQueue, BufferConsumer, BufferProducer};

use extra_impls::mpsc_queue::Queue as MpscQueue;

mod extra_impls;

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

struct SpscDummy<T: Send> {
    co: BufferConsumer<T>,
    pr: BufferProducer<T>,
}

impl<T: Send> SpscDummy<T> {
    pub fn new(s: usize) -> SpscDummy<T> {
        let (pr, co) = SpscBufferQueue::new(s);
        SpscDummy {
            pr: pr,
            co: co,
        }
    }
}

unsafe impl<T: Send> Sync for SpscDummy<T> {}

trait Queue<T> {
    fn push(&self, T);
    fn try_pop(&self) -> Option<T>;
}

impl<T> Queue<T> for MsQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn try_pop(&self) -> Option<T> { self.try_pop() }
}

impl<T> Queue<T> for SegQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn try_pop(&self) -> Option<T> { self.try_pop() }
}

impl<T> Queue<T> for MpscQueue<T> {
    fn push(&self, t: T) { self.push(t) }
    fn try_pop(&self) -> Option<T> {
        use extra_impls::mpsc_queue::*;

        loop {
            match self.pop() {
                Data(t) => return Some(t),
                Empty => return None,
                Inconsistent => (),
            }
        }
    }
}

impl<T: Send> Queue<T> for SpscDummy<T> {
    fn push(&self, val: T) {
        let mut temp = val;
        loop {
            match self.pr.try_push(temp) {
                Ok(_) => break,
                Err(rval) => temp=rval,
            }
        }
    }
    fn try_pop(&self) -> Option<T> {
        self.co.try_pop()
    }
}

impl<T> Queue<T> for Mutex<VecDeque<T>> {
    fn push(&self, t: T) { self.lock().unwrap().push_back(t) }
    fn try_pop(&self) -> Option<T> { self.lock().unwrap().pop_front() }
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
            while count < COUNT*THREADS {
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
                scope.spawn(move || {
                    loop {
                        if let Some(false) = qr.try_pop() { break }
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

            for _i in 0..COUNT*THREADS {
                let _ = rx.recv().unwrap();
            }
        });
    });

    nanos(d) / ((COUNT * THREADS) as f64)
}

fn bench_queue_spsc<Q: Queue<u64> + Sync>(q: Q, scale: u64) -> f64 {
    let d = time(||{
        let qr = &q;
        scope(|scope| {
            scope.spawn(move ||{
                for x in 0..COUNT*scale {
                    qr.push(x)
                }
            });
            for _ in 0..COUNT*scale {
                loop {
                    if let Some(_) = q.try_pop() {
                        break;
                    }
                }
            }
        });
    });
    nanos(d) / ((COUNT*scale) as f64)
}

fn main() {
    println!("MSQ mpsc: {}", bench_queue_mpsc(MsQueue::new()));
    println!("chan mpsc: {}", bench_chan_mpsc());
    println!("mpsc mpsc: {}", bench_queue_mpsc(MpscQueue::new()));
    println!("Seg mpsc: {}", bench_queue_mpsc(SegQueue::new()));

    println!("MSQ mpmc: {}", bench_queue_mpmc(MsQueue::new()));
    println!("Seg mpmc: {}", bench_queue_mpmc(SegQueue::new()));

    println!("BufferQueue spsc: {}", bench_queue_spsc(SpscDummy::new(512), 100));
    println!("Seg spsc: {}", bench_queue_spsc(SegQueue::new(), 1));
    println!("Ms spsc: {}", bench_queue_spsc(MsQueue::new(), 1));
    println!("mpsc spsc: {}", bench_queue_spsc(MpscQueue::new(), 1));

//    println!("queue_mpsc: {}", bench_queue_mpsc());
//    println!("queue_mpmc: {}", bench_queue_mpmc());
//   println!("mutex_mpmc: {}", bench_mutex_mpmc());
}
