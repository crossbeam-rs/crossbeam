use std::sync::atomic::{AtomicUsize, Ordering};

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
fn drain_end_exceeds_len() {
    // range extends beyond queue length — should not panic
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(..100).collect();
    assert_eq!(drained, (0..10).collect::<Vec<_>>());
    assert!(q.is_empty());
}

#[test]
fn drain_empty_range() {
    // drain(..0) - drain nothing
    let mut q = SegQueue::new();
    for i in 0..10 {
        q.push(i);
    }
    let drained: Vec<i32> = q.drain(..0).collect();
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
#[should_panic(expected = "end index overflow")]
fn drain_inclusive_usize_max() {
    let mut q = SegQueue::<i32>::new();
    q.drain(..=usize::MAX);
}

#[test]
fn drain_mem_forget() {
    // If the Drain is leaked (e.g. via mem::forget), the elements that were
    // not yet yielded simply remain in the queue.
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug)]
    struct DropCounter;
    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    {
        let mut q = SegQueue::new();
        for _ in 0..100 {
            q.push(DropCounter);
        }
        let mut drain = q.drain(..);
        drain.next(); // consume 1
        std::mem::forget(drain);
        // the other 99 elements are still in the queue, which remains usable
        assert_eq!(q.len(), 99);
        q.push(DropCounter);
        assert_eq!(q.len(), 100);
    }
    // 1 from next() + 100 dropped with the queue. Nothing is leaked.
    assert_eq!(DROPS.load(Ordering::SeqCst), 101);
}
