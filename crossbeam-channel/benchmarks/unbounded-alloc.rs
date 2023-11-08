use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam::channel::unbounded;

mod message;

const MESSAGES: usize = 31 * 160_000;
const THREADS: usize = 4;

struct Counter;

static NUM_ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        NUM_ALLOCS.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static A: Counter = Counter;

fn spsc() -> usize {
    let (tx, rx) = unbounded();
    let before = NUM_ALLOCS.load(Ordering::Relaxed);
    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES / 31 {
                for _ in 0..31 {
                    tx.send(message::new(i)).unwrap();
                }
                std::thread::yield_now();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    })
    .unwrap();
    NUM_ALLOCS.load(Ordering::Relaxed) - before
}

fn mpsc() -> usize {
    let (tx, rx) = unbounded();
    let before = NUM_ALLOCS.load(Ordering::Relaxed);
    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS / 31 {
                    for _ in 0..31 {
                        tx.send(message::new(i)).unwrap();
                    }
                    std::thread::yield_now();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    })
    .unwrap();
    NUM_ALLOCS.load(Ordering::Relaxed) - before
}

fn spmc() -> usize {
    let (tx, rx) = unbounded();
    let before = NUM_ALLOCS.load(Ordering::Relaxed);
    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES / 31 {
                for _ in 0..31 {
                    tx.send(message::new(i)).unwrap();
                }
                std::thread::yield_now();
            }
        });

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
                }
            });
        }
    })
    .unwrap();
    NUM_ALLOCS.load(Ordering::Relaxed) - before
}

fn mpmc() -> usize {
    let (tx, rx) = unbounded();
    let before = NUM_ALLOCS.load(Ordering::Relaxed);
    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..MESSAGES / THREADS / 31 {
                    for _ in 0..31 {
                        tx.send(message::new(i)).unwrap();
                    }
                    std::thread::yield_now();
                }
            });
        }

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
                }
            });
        }
    })
    .unwrap();
    NUM_ALLOCS.load(Ordering::Relaxed) - before
}

fn run(name: &str, n_allocs: usize) {
    println!("{} | allocs: {}", name, n_allocs);
}

fn main() {
    run("spsc", spsc());
    run("spsc", spsc());
    run("mpsc", mpsc());
    run("mpsc", mpsc());
    run("spmc", spmc());
    run("spmc", spmc());
    run("mpmc", mpmc());
    run("mpmc", mpmc());
}
