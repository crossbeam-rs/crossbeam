extern crate crossbeam_epoch as epoch;
extern crate rand;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
use std::time::{Duration, Instant};
use std::thread;

use epoch::{Atomic, Owned};
use rand::Rng;

fn worker(a: Arc<Atomic<AtomicUsize>>) -> usize {
    let mut rng = rand::thread_rng();
    let mut sum = 0;

    thread::sleep(Duration::from_millis(rng.gen_range(0, 1_000)));
    let timeout = Duration::from_millis(rng.gen_range(0, 1_000));
    let now = Instant::now();

    while now.elapsed() < timeout {
        for _ in 0..100 {
            let guard = &epoch::pin();

            let val = if rng.gen_range(0, 10) == 0 {
                let p = a.swap(Owned::new(AtomicUsize::new(sum)), AcqRel, guard);
                unsafe {
                    guard.defer(move || p.into_owned());
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
    // Pin the current thread in order to initialize the global collector, otherwise tsan will show
    // false positives coming from `lazy_static!`.
    epoch::pin();

    for _ in 0..10 {
        let a = Arc::new(Atomic::new(AtomicUsize::new(777)));

        let threads = (0..50)
            .map(|_| {
                let a = a.clone();
                thread::spawn(move || worker(a))
            })
            .collect::<Vec<_>>();

        for t in threads {
            t.join().unwrap();
        }
    }
}
