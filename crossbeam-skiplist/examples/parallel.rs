use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crossbeam_skiplist::SkipSet;
use rand::RngExt;

fn main() {
    let results: Vec<_> = std::env::args()
        .skip(1)
        .map(|nthreads| {
            // Run 30 iterations with 1_048_576 keys and 10_000_000 ops per thread
            let result = run(30, nthreads.parse().unwrap(), 1_048_576, 10_000_000);
            (nthreads, result)
        })
        .collect();
    println!("threads,throughput");
    for (nthreads, throughput) in results {
        println!("{nthreads},{throughput:.0}");
    }
}

fn run(iters: usize, nthreads: usize, nkeys: usize, ops_per_thread: usize) -> f32 {
    let total_runtime = Mutex::new(Duration::ZERO);

    eprintln!(
        "running {} operations per thread ({} threads, {} total operations)...",
        ops_per_thread,
        nthreads,
        ops_per_thread * nthreads,
    );

    for i in 0..iters {
        // Initialize barrier (barrier == 2 * nthreads)
        let barrier = AtomicUsize::new(2 * nthreads);
        let set: SkipSet<usize> = SkipSet::new();

        // Fill the set to 50% by inserting all even-numbered keys
        for x in 0..(nkeys / 2) {
            set.insert(x * 2);
        }

        std::thread::scope(|s| {
            for tid in 0..nthreads {
                let barrier = &barrier;
                let set = &set;
                let total_runtime = &total_runtime;
                s.spawn(move || {
                    let mut rng = rand::rng();

                    // Wait for all threads to reach here (barrier == nthreads)
                    barrier.fetch_sub(1, Ordering::Relaxed);
                    while barrier.load(Ordering::Relaxed) > nthreads {
                        std::hint::spin_loop();
                    }
                    let start = Instant::now();

                    // BEGIN benchmarked region
                    for _ in 0..ops_per_thread {
                        // Pick a random key and random operation with equal likelihood: insert, remove, or contains
                        let key = rng.random_range(0..nkeys);
                        let operation = rng.random_range(0..3);
                        match operation {
                            0 => {
                                set.insert(key);
                            }
                            1 => {
                                set.remove(&key);
                            }
                            _ => {
                                set.contains(&key);
                            }
                        };
                    }
                    // END benchmarked region

                    barrier.fetch_sub(1, Ordering::Relaxed);

                    if tid == 0 {
                        // Wait for all threads to complete (barrier == 0)
                        while barrier.load(Ordering::Relaxed) != 0 {
                            std::hint::spin_loop();
                        }

                        let elapsed = start.elapsed();
                        eprintln!(
                            "iter {:2}: {:.4?}, throughput {:.2} ops/s",
                            i + 1,
                            elapsed,
                            (ops_per_thread * nthreads) as f32 / elapsed.as_secs_f32(),
                        );
                        *total_runtime.lock().unwrap() += elapsed;
                    }
                });
            }
        });
    }

    let average_time = total_runtime.into_inner().unwrap() / iters as u32;
    let throughput = (ops_per_thread * nthreads) as f32 / average_time.as_secs_f32();
    eprintln!("\naverage time: {average_time:?}, throughput {throughput} ops/s");
    throughput
}
