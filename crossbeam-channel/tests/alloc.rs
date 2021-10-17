//! Tests which require a global allocator that counts allocations.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crossbeam_channel::{bounded, unbounded};
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

    let (ready_s, ready_r) = bounded(1);
    let before = NUM_ALLOCS.load(Ordering::SeqCst);
    scope(|scope| {
        scope.spawn(|_| {
            for i in 0..100_000 {
                for _ in 0..31 {
                    s.send(i as i32).unwrap();
                }
                ready_r.recv().unwrap();
            }
        });
        for _ in 0..100_000 {
            for _ in 0..31 {
                r.recv().unwrap();
            }
            ready_s.send(()).unwrap();
        }
    })
    .unwrap();
    let after = NUM_ALLOCS.load(Ordering::SeqCst);
    // Exactly 19 allocations are made every time on my box. Let's say 50 to be
    // safe.
    assert!(before + 50 > after);
}
