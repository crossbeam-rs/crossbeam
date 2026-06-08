use std::{
    ops::Bound,
    sync::atomic::{AtomicUsize, Ordering},
};

use crossbeam_queue::SegQueue;
use crossbeam_utils::thread::scope;

#[test]
fn smoke() {
    let q = SegQueue::new();
    q.push(7);
    assert_eq!(q.pop(), Some(7));

    q.push(8);
    assert_eq!(q.pop(), Some(8));
    assert!(q.pop().is_none());
}

#[test]
fn len_empty_full() {
    let q = SegQueue::new();

    assert_eq!(q.len(), 0);
    assert!(q.is_empty());

    q.push(());

    assert_eq!(q.len(), 1);
    assert!(!q.is_empty());

    q.pop().unwrap();

    assert_eq!(q.len(), 0);
    assert!(q.is_empty());
}

#[test]
fn len() {
    let q = SegQueue::new();

    assert_eq!(q.len(), 0);

    for i in 0..50 {
        q.push(i);
        assert_eq!(q.len(), i + 1);
    }

    for i in 0..50 {
        q.pop().unwrap();
        assert_eq!(q.len(), 50 - i - 1);
    }

    assert_eq!(q.len(), 0);
}

#[test]
fn exclusive_reference() {
    let mut q = SegQueue::new();

    assert_eq!(q.len(), 0);

    for i in 0..50 {
        q.push_mut(i);
        assert_eq!(q.len(), i + 1);
    }

    for i in 0..50 {
        q.pop_mut().unwrap();
        assert_eq!(q.len(), 50 - i - 1);
    }

    assert_eq!(q.len(), 0);
    assert!(q.is_empty());

    for i in 0..35 {
        q.push(i);
        assert_eq!(q.len(), i + 1);
    }

    for i in 0..5 {
        q.push_mut(i);
        assert_eq!(q.len(), 35 + i + 1);
    }

    for i in 0..5 {
        q.pop_mut().unwrap();
        assert_eq!(q.len(), 40 - i - 1);
    }

    for i in 0..35 {
        q.pop().unwrap();
        assert_eq!(q.len(), 35 - i - 1);
    }

    assert_eq!(q.len(), 0);
    assert!(q.is_empty());

    q.push_mut(1);

    assert!(!q.is_empty());
}

#[test]
fn spsc() {
    const COUNT: usize = if cfg!(miri) { 100 } else { 100_000 };

    let q = SegQueue::new();

    scope(|scope| {
        scope.spawn(|_| {
            for i in 0..COUNT {
                loop {
                    if let Some(x) = q.pop() {
                        assert_eq!(x, i);
                        break;
                    }
                }
            }
            assert!(q.pop().is_none());
        });
        scope.spawn(|_| {
            for i in 0..COUNT {
                q.push(i);
            }
        });
    })
    .unwrap();
}

#[test]
fn mpmc() {
    const COUNT: usize = if cfg!(miri) { 50 } else { 25_000 };
    const THREADS: usize = 4;

    let q = SegQueue::<usize>::new();
    let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

    scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..COUNT {
                    let n = loop {
                        if let Some(x) = q.pop() {
                            break x;
                        }
                    };
                    v[n].fetch_add(1, Ordering::SeqCst);
                }
            });
        }
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..COUNT {
                    q.push(i);
                }
            });
        }
    })
    .unwrap();

    for c in v {
        assert_eq!(c.load(Ordering::SeqCst), THREADS);
    }
}

#[test]
fn drops() {
    let runs: usize = if cfg!(miri) { 5 } else { 100 };
    let steps: usize = if cfg!(miri) { 50 } else { 10_000 };
    let additional: usize = if cfg!(miri) { 100 } else { 1_000 };

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug, PartialEq)]
    struct DropCounter;

    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut rng = fastrand::Rng::new();

    for _ in 0..runs {
        let steps = rng.usize(0..steps);
        let additional = rng.usize(0..additional);

        DROPS.store(0, Ordering::SeqCst);
        let q = SegQueue::new();

        scope(|scope| {
            scope.spawn(|_| {
                for _ in 0..steps {
                    while q.pop().is_none() {}
                }
            });

            scope.spawn(|_| {
                for _ in 0..steps {
                    q.push(DropCounter);
                }
            });
        })
        .unwrap();

        for _ in 0..additional {
            q.push(DropCounter);
        }

        assert_eq!(DROPS.load(Ordering::SeqCst), steps);
        drop(q);
        assert_eq!(DROPS.load(Ordering::SeqCst), steps + additional);
    }
}

#[test]
fn into_iter() {
    let q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    for (i, j) in q.into_iter().enumerate() {
        assert_eq!(i, j);
    }
}

#[test]
fn into_iter_drop() {
    let q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    for (i, j) in q.into_iter().enumerate().take(50) {
        assert_eq!(i, j);
    }
}

// If `Block` is created on the stack, the array of slots will multiply this `BigStruct` and
// probably overflow the thread stack. It's now directly created on the heap to avoid this.
#[test]
fn stack_overflow() {
    const N: usize = 32_768;
    struct BigStruct {
        _data: [u8; N],
    }

    let q = SegQueue::new();
    q.push(BigStruct { _data: [0u8; N] });

    for _data in q.into_iter() {}
}

#[test]
fn drain_full() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push_mut(i);
    }
    for (i, j) in q.drain(..).enumerate() {
        assert_eq!(i, j);
    }
    assert!(q.is_empty());
    assert_eq!(q.len(), 0);
}

#[test]
fn drain_empty() {
    let mut q = SegQueue::<i32>::new();
    let drained: Vec<i32> = q.drain(..).collect();
    assert!(drained.is_empty());
    assert!(q.is_empty());
}

#[test]
fn drain_full_drop() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    {
        let mut drain = q.drain(..);
        for i in 0..50 {
            assert_eq!(drain.next(), Some(i));
        }
    }
    assert!(q.is_empty());
    assert_eq!(q.len(), 0);
    q.push(42);
    assert_eq!(q.pop_mut(), Some(42));
}

#[test]
fn drain_drops() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug)]
    struct DropCounter;
    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    // Case 1: fully consume drain(..)
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        let _: Vec<_> = q.drain(..).collect();
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), 100);

    // Case 2: drop drain(..) mid-way
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        {
            let mut drain = q.drain(..);
            for _ in 0..30 {
                drain.next();
            }
        }
        assert_eq!(DROPS.load(Ordering::SeqCst), 100);
        assert!(q.is_empty());
    }

    // Case 3: fully consume drain(..n)
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        let _: Vec<_> = q.drain(..60).collect();
        assert_eq!(DROPS.load(Ordering::SeqCst), 60);
        assert_eq!(q.len(), 40);
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), 100);

    // Case 4: drop drain(..n) mid-way
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        {
            let mut drain = q.drain(..60);
            for _ in 0..20 {
                drain.next();
            }
        }
        assert_eq!(DROPS.load(Ordering::SeqCst), 60);
        assert_eq!(q.len(), 40);
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), 100);

    // Case 5: fully consume drain(a..b)
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        let _: Vec<_> = q.drain(20..70).collect();
        // 50 drained, 50 remain (20 prefix + 30 suffix)
        assert_eq!(DROPS.load(Ordering::SeqCst), 50);
        assert_eq!(q.len(), 50);
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), 100);

    // Case 6: drop drain(a..b) mid-way
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        {
            let mut drain = q.drain(20..70);
            for _ in 0..10 {
                drain.next();
            }
            // drop — remaining 40 of range dropped, 50 outside range kept
        }
        assert_eq!(DROPS.load(Ordering::SeqCst), 50);
        assert_eq!(q.len(), 50);
    }
    assert_eq!(DROPS.load(Ordering::SeqCst), 100);
}

#[test]
fn drain_prefix() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(..60).collect();
    assert_eq!(drained, (0..60).collect::<Vec<_>>());
    assert_eq!(q.len(), 40);
    for i in 60..100 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_prefix_drop() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    {
        let mut drain = q.drain(..50);
        for i in 0..20 {
            assert_eq!(drain.next(), Some(i));
        }
    }
    assert_eq!(q.len(), 50);
    for i in 50..100 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_prefix_exact() {
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(..=4).collect();
    assert_eq!(drained, [0, 1, 2, 3, 4]);
    assert_eq!(q.len(), 5);
    for i in 5..10 {
        assert_eq!(q.pop_mut(), Some(i));
    }
}

#[test]
fn drain_range() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(20..70).collect();
    assert_eq!(drained, (20..70).collect::<Vec<_>>());
    assert_eq!(q.len(), 50);
    // prefix intact
    for i in 0..20 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    // suffix intact
    for i in 70..100 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_range_drop() {
    // Drop drain(a..b) mid-way — prefix and suffix both preserved
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    {
        let mut drain = q.drain(20..70);
        for i in 20..35 {
            assert_eq!(drain.next(), Some(i));
        }
        // drop — elements 35..70 dropped, 0..20 and 70..100 stay
    }
    assert_eq!(q.len(), 50);
    for i in 0..20 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    for i in 70..100 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_range_inclusive() {
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(2..=5).collect();
    assert_eq!(drained, [2, 3, 4, 5]);
    assert_eq!(q.len(), 6);
    for i in [0, 1, 6, 7, 8, 9] {
        assert_eq!(q.pop_mut(), Some(i));
    }
}

#[test]
fn drain_range_start_only() {
    // drain(a..) — skip prefix, drain everything else
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(30..).collect();
    assert_eq!(drained, (30..100).collect::<Vec<_>>());
    assert_eq!(q.len(), 30);
    for i in 0..30 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_range_exceeds_len() {
    // range extends beyond queue length — should not panic
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(5..100).collect();
    assert_eq!(drained, (5..10).collect::<Vec<_>>());
    assert_eq!(q.len(), 5);
    for i in 0..5 {
        assert_eq!(q.pop_mut(), Some(i));
    }
}

#[test]
fn drain_range_empty_range() {
    // drain(n..n) — drain nothing
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(5..5).collect();
    assert!(drained.is_empty());
    assert_eq!(q.len(), 10);
    for i in 0..10 {
        assert_eq!(q.pop_mut(), Some(i));
    }
}

#[test]
fn drain_block_boundary() {
    // BLOCK_CAP=31, drain across multiple block boundaries
    let mut q = SegQueue::new();
    for i in 0..200 {
        q.push(i);
    }
    for (i, j) in q.drain(..).enumerate() {
        assert_eq!(i, j);
    }
    assert!(q.is_empty());
    for i in 0..50 {
        q.push(i);
    }
    assert_eq!(q.len(), 50);
}

#[test]
fn drain_prefix_block_boundary() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    // crosses first block boundary at 31
    let drained: Vec<i32> = q.drain(..40).collect();
    assert_eq!(drained, (0..40).collect::<Vec<_>>());
    assert_eq!(q.len(), 60);
    for i in 40..100 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_range_block_boundary() {
    // range that starts and ends across block boundaries
    let mut q = SegQueue::new();
    for i in 0..200 {
        q.push(i);
    }
    // crosses boundaries at 31, 62, 93...
    let drained: Vec<i32> = q.drain(25..95).collect();
    assert_eq!(drained, (25..95).collect::<Vec<_>>());
    assert_eq!(q.len(), 130);
    for i in 0..25 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    for i in 95..200 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_then_reuse() {
    let mut q = SegQueue::new();
    for i in 0..50 {
        q.push(i);
    }
    let _: Vec<_> = q.drain(..).collect();
    assert!(q.is_empty());
    for i in 0..50 {
        q.push(i);
    }
    for i in 0..50 {
        assert_eq!(q.pop(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_prefix_then_reuse() {
    let mut q = SegQueue::new();
    for i in 0..50 {
        q.push(i);
    }
    let _: Vec<_> = q.drain(..25).collect();
    assert_eq!(q.len(), 25);
    for i in 100..110 {
        q.push(i);
    }
    for i in 25..50 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    for i in 100..110 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
fn drain_exact_size() {
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }

    let drain = q.drain(..60);
    assert_eq!(drain.len(), 60);
    drop(drain);

    let drain = q.drain(..);
    assert_eq!(drain.len(), 40);
    drop(drain);

    // drain(a..b)
    let mut q = SegQueue::new();
    for i in 0..100 {
        q.push(i);
    }
    let drain = q.drain(20..70);
    assert_eq!(drain.len(), 50);
    drop(drain);

    // size_hint when range exceeds queue length
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drain = q.drain(..100);
    assert_eq!(drain.len(), 10);
    drop(drain);
}

#[test]
fn drain_range_then_reuse() {
    let mut q = SegQueue::new();
    for i in 0..50 {
        q.push(i);
    }
    let _: Vec<_> = q.drain(10..40).collect();
    assert_eq!(q.len(), 20);
    for i in 100..110 {
        q.push(i);
    }
    for i in 0..10 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    for i in 40..50 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    for i in 100..110 {
        assert_eq!(q.pop_mut(), Some(i));
    }
    assert!(q.is_empty());
}

#[test]
#[should_panic(expected = "end index overflow")]
fn drain_inclusive_usize_max() {
    let mut q = SegQueue::<i32>::new();
    q.drain(..=usize::MAX);
}

#[test]
#[should_panic(expected = "start index overflow")]
fn drain_excluded_start_usize_max() {
    let mut q = SegQueue::<i32>::new();
    q.drain((Bound::Excluded(usize::MAX), Bound::Unbounded));
}

// This test intentionally leaks memory via `mem::forget` to verify that
// the queue remains in a consistent (empty) state after a `Drain` is forgotten.
// Skipped under Miri due to intentional leaks; LeakSanitizer may also flag this test.
#[cfg_attr(miri, ignore)]
#[test]
fn drain_mem_forget() {
    // If mem::forget is called on Drain, queue must be left in
    // a consistent (empty) state. Elements are leaked but no corruption.
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug)]
    struct DropCounter;
    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    // Case 1: mem::forget on drain(..)
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        let mut drain = q.drain(..);
        drain.next(); // consume 1
        std::mem::forget(drain);
        // queue must be empty and usable
        assert!(q.is_empty());
        q.push(DropCounter);
        assert_eq!(q.len(), 1);
    }
    // 1 from next() + 1 from q drop. remaining 99 are leaked.
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);

    // Case 2: mem::forget on drain(a..b)
    DROPS.store(0, Ordering::SeqCst);
    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        let mut drain = q.drain(20..70);
        drain.next(); // consume 1
        std::mem::forget(drain);
        // queue must be empty and usable
        assert!(q.is_empty());
        q.push(DropCounter);
        assert_eq!(q.len(), 1);
    }
    // 1 from next() + 1 from q drop. remaining 99 are leaked.
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);
}
