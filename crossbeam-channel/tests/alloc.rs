//! Tests which require a global allocator that counts allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::{bounded, unbounded};
use crossbeam_utils::thread::scope;
use rand::{thread_rng, Rng};

const BLOCK_CAP: usize = 31;

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

#[test]
fn allocs() {
    let (s, r) = unbounded();

    // We expect to see the first additional allocation after exactly BLOCK_CAP sends and receives.
    let before = NUM_ALLOCS.load(Ordering::Relaxed);
    for i in 0..BLOCK_CAP - 1 {
        s.send(i as i32).unwrap();
        r.recv().unwrap();
    }
    let after = NUM_ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before, after);
    s.send((BLOCK_CAP - 1) as i32).unwrap();
    r.recv().unwrap();
    let after = NUM_ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before + 1, after);

    // Warm up the cache by sending BLOCK_CAP * 4  more messages.
    let before = after;
    for i in 0..BLOCK_CAP * 4 {
        s.send(i as i32).unwrap();
    }
    for _ in 0..BLOCK_CAP * 4 {
        r.recv().unwrap();
    }
    let after = NUM_ALLOCS.load(Ordering::Relaxed);
    assert_eq!(before + 3, after);

    // Now we can send up to BLOCK_CAP * 4 messages before incurring an allocation.
    let mut rng = thread_rng();
    let (count_s, count_r) = bounded(1);
    scope(|scope| {
        scope.spawn(|_| {
            // Get the waker allocations out of the way before starting the test.
            for _ in 0..8 {
                count_r.recv().unwrap();
            }

            let before = NUM_ALLOCS.load(Ordering::Relaxed);
            for i in 0..100_000 {
                let count = count_r.recv().unwrap();
                for _ in 0..count {
                    s.send(i as i32).unwrap();
                }
            }
            let after = NUM_ALLOCS.load(Ordering::Relaxed);
            assert_eq!(before, after);
        });

        // Get the waker allocations out of the way before starting the test.
        let _ = r.recv_timeout(std::time::Duration::from_millis(10));
        for i in 0..8 {
            count_s.send(i).unwrap();
        }

        for i in 0..100_000 {
            let count = if i % 2 == 0 {
                BLOCK_CAP * 4
            } else {
                rng.gen_range(1..BLOCK_CAP * 4)
            };
            count_s.send(count).unwrap();
            for _ in 0..count {
                r.recv().unwrap();
            }
        }
    })
    .unwrap();
}
