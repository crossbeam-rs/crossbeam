extern crate crossbeam_epoch as epoch;
extern crate rand;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
use std::time::{Duration, Instant};
use std::thread;

use epoch::{Atomic, Collector, Handle, Owned, Shared};
use rand::Rng;

fn worker(a: Arc<Atomic<AtomicUsize>>, handle: Handle) -> usize {
    let mut rng = rand::thread_rng();
    let mut sum = 0;

    if rng.gen() {
        thread::sleep(Duration::from_millis(1));
    }
    let timeout = Duration::from_millis(rng.gen_range(0, 10));
    let now = Instant::now();

    while now.elapsed() < timeout {
        for _ in 0..100 {
            let guard = &handle.pin();
            guard.flush();

            let val = if rng.gen() {
                let p = a.swap(Owned::new(AtomicUsize::new(sum)), AcqRel, guard);
                unsafe {
                    guard.defer(move || p.into_owned());
                    guard.flush();
                    p.deref().load(Relaxed)
                }
            } else {
                let p = a.load(Acquire, guard);
                unsafe {
                    p.deref().fetch_add(sum, Relaxed)
                }
            };

            sum = sum.wrapping_add(val);
        }
    }

    sum
}

fn main() {
    for _ in 0..100 {
        let collector = Collector::new();
        let a = Arc::new(Atomic::new(AtomicUsize::new(777)));

        let threads = (0..16)
            .map(|_| {
                let a = a.clone();
                let h = collector.handle();
                thread::spawn(move || worker(a, h))
            })
            .collect::<Vec<_>>();

        for t in threads {
            t.join().unwrap();
        }

        unsafe {
            a.swap(Shared::null(), AcqRel, epoch::unprotected()).into_owned();
        }
    }
}
