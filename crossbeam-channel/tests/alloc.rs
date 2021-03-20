//! Tests which require a global allocator that counts allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crossbeam_channel::unbounded;
use crossbeam_utils::thread::scope;

struct Counter;

static NUM_ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ret = System.alloc(layout);
        if !ret.is_null() {
            NUM_ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        return ret;
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

    // We expect to see the first (and only) additional allocation after exactly
    // BLOCK_CAP sends and receives.
    let before = NUM_ALLOCS.load(Ordering::SeqCst);
    for i in 0..30 {
        s.send(i as i32).unwrap();
        r.recv().unwrap();
    }
    let after = NUM_ALLOCS.load(Ordering::SeqCst);
    assert_eq!(after, before);
    s.send(1).unwrap();
    r.recv().unwrap();
    let after = NUM_ALLOCS.load(Ordering::SeqCst);
    assert_eq!(after, before + 1);

    // Now we can send up to BLOCK_CAP messages before incurring an allocation.
    for _ in 0..100 {
        let before = NUM_ALLOCS.load(Ordering::SeqCst);
        for i in 0..31 {
            s.send(i as i32).unwrap();
        }
        for _ in 0..31 {
            r.recv().unwrap();
        }
        let after = NUM_ALLOCS.load(Ordering::SeqCst);
        assert_eq!(before, after);
    }

    let before = NUM_ALLOCS.load(Ordering::SeqCst);
    scope(|scope| {
        scope.spawn(|_| {
            for i in 0..31 * 1_000_000 {
                s.send(i as i32).unwrap();
            }
        });
        for _ in 0..31 * 1_000_000 {
            r.recv().unwrap();
        }
    })
    .unwrap();
    let after = NUM_ALLOCS.load(Ordering::SeqCst);
    assert!(after < before + 500_000);

    let threads = num_cpus::get();
    let before = NUM_ALLOCS.load(Ordering::SeqCst);
    scope(|scope| {
        for _ in 0..threads / 2 {
            scope.spawn(|_| {
                for i in 0..31 * 100_000 {
                    s.send(i as i32).unwrap();
                }
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
        for _ in 0..threads / 2 {
            for _ in 0..31 * 100_000 {
                r.recv().unwrap();
            }
        }
    })
    .unwrap();
    let after = NUM_ALLOCS.load(Ordering::SeqCst);
    assert!(after < before + ((threads / 4) * 100_000));
}
