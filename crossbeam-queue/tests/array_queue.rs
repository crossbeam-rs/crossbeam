use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_queue::ArrayQueue;
use crossbeam_utils::thread::scope;

#[test]
fn smoke() {
    let q = ArrayQueue::new(1);

    q.push(7).unwrap();
    assert_eq!(q.pop(), Some(7));

    q.push(8).unwrap();
    assert_eq!(q.pop(), Some(8));
    assert!(q.pop().is_none());
}

#[test]
fn capacity() {
    for i in 1..10 {
        let q = ArrayQueue::<i32>::new(i);
        assert_eq!(q.capacity(), i);
    }
}

#[test]
fn fallible_capacity_and_fifo() {
    for capacity in [1, 2, 3, 63, 64, 65, 256] {
        let queue = ArrayQueue::try_new(capacity).unwrap();
        assert_eq!(queue.capacity(), capacity);
        // Repeated fills exercise wraparound for power-of-two and other capacities.
        for _ in 0..3 {
            for value in 0..capacity {
                queue.push(value).unwrap();
            }
            assert_eq!(queue.push(capacity), Err(capacity));
            for value in 0..capacity {
                assert_eq!(queue.pop(), Some(value));
            }
            assert!(queue.is_empty());
        }
    }
}

#[test]
fn fallible_capacity_overflow() {
    assert!(ArrayQueue::<usize>::try_new(usize::MAX).is_err());
    assert!(ArrayQueue::<()>::try_new(usize::MAX).is_err());
}

#[test]
#[should_panic(expected = "capacity must be non-zero")]
fn fallible_zero_capacity() {
    let _ = ArrayQueue::<usize>::try_new(0);
}

#[test]
fn fallible_zero_sized_and_aligned_elements() {
    #[derive(Debug, PartialEq)]
    #[repr(align(256))]
    struct Aligned(u8);

    for capacity in [1, 3, 64] {
        let empty = ArrayQueue::<()>::try_new(capacity).unwrap();
        let aligned = ArrayQueue::try_new(capacity).unwrap();
        for value in 0..capacity {
            empty.push(()).unwrap();
            aligned.push(Aligned(value as u8)).unwrap();
        }
        assert_eq!(empty.push(()), Err(()));
        assert_eq!(aligned.push(Aligned(0)), Err(Aligned(0)));
        for value in 0..capacity {
            assert_eq!(empty.pop(), Some(()));
            assert_eq!(aligned.pop(), Some(Aligned(value as u8)));
        }
        assert!(empty.is_empty());
        assert!(aligned.is_empty());
    }
}

#[test]
fn fallible_wrapped_drop_and_into_iter() {
    use std::sync::Arc;

    #[derive(Debug)]
    struct DropCounter(Arc<AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drops = Arc::new(AtomicUsize::new(0));
    let queue = ArrayQueue::try_new(3).unwrap();
    for _ in 0..3 {
        queue.push(DropCounter(drops.clone())).unwrap();
    }
    drop(queue.pop());
    queue.push(DropCounter(drops.clone())).unwrap();
    let mut iter = queue.into_iter();
    drop(iter.next());
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    drop(iter);
    assert_eq!(drops.load(Ordering::SeqCst), 4);
}

#[test]
#[should_panic(expected = "capacity must be non-zero")]
fn zero_capacity() {
    let _ = ArrayQueue::<i32>::new(0);
}

#[test]
fn len_empty_full() {
    let q = ArrayQueue::new(2);

    assert_eq!(q.len(), 0);
    assert!(q.is_empty());
    assert!(!q.is_full());

    q.push(()).unwrap();

    assert_eq!(q.len(), 1);
    assert!(!q.is_empty());
    assert!(!q.is_full());

    q.push(()).unwrap();

    assert_eq!(q.len(), 2);
    assert!(!q.is_empty());
    assert!(q.is_full());

    q.pop().unwrap();

    assert_eq!(q.len(), 1);
    assert!(!q.is_empty());
    assert!(!q.is_full());
}

#[test]
fn exclusive_reference() {
    let mut q = ArrayQueue::new(2);

    assert_eq!(q.len(), 0);
    assert!(q.is_empty());

    q.push_mut(()).unwrap();

    assert_eq!(q.len(), 1);
    assert!(!q.is_empty());
    assert!(!q.is_full());

    q.push_mut(()).unwrap();

    assert_eq!(q.len(), 2);
    assert!(!q.is_empty());
    assert!(q.is_full());

    q.pop_mut().unwrap();

    assert_eq!(q.len(), 1);
    assert!(!q.is_empty());
    assert!(!q.is_full());

    q.pop().unwrap();

    assert_eq!(q.len(), 0);
}

#[test]
fn len() {
    const COUNT: usize = if cfg!(miri) { 30 } else { 25_000 };
    const CAP: usize = if cfg!(miri) { 40 } else { 1000 };
    const ITERS: usize = CAP / 20;

    let q = ArrayQueue::new(CAP);
    assert_eq!(q.len(), 0);

    for _ in 0..CAP / 10 {
        for i in 0..ITERS {
            q.push(i).unwrap();
            assert_eq!(q.len(), i + 1);
        }

        for i in 0..ITERS {
            q.pop().unwrap();
            assert_eq!(q.len(), ITERS - i - 1);
        }
    }
    assert_eq!(q.len(), 0);

    for i in 0..CAP {
        q.push(i).unwrap();
        assert_eq!(q.len(), i + 1);
    }

    for _ in 0..CAP {
        q.pop().unwrap();
    }
    assert_eq!(q.len(), 0);

    scope(|scope| {
        scope.spawn(|_| {
            for i in 0..COUNT {
                loop {
                    if let Some(x) = q.pop() {
                        assert_eq!(x, i);
                        break;
                    }
                }
                let len = q.len();
                assert!(len <= CAP);
            }
        });

        scope.spawn(|_| {
            for i in 0..COUNT {
                while q.push(i).is_err() {}
                let len = q.len();
                assert!(len <= CAP);
            }
        });
    })
    .unwrap();
    assert_eq!(q.len(), 0);
}

#[test]
fn spsc() {
    const COUNT: usize = if cfg!(miri) { 50 } else { 100_000 };

    let q = ArrayQueue::new(3);

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
                while q.push(i).is_err() {}
            }
        });
    })
    .unwrap();
}

#[test]
fn spsc_ring_buffer() {
    const COUNT: usize = if cfg!(miri) { 50 } else { 100_000 };

    let t = AtomicUsize::new(1);
    let q = ArrayQueue::<usize>::new(3);
    let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

    scope(|scope| {
        scope.spawn(|_| {
            loop {
                match t.load(Ordering::SeqCst) {
                    0 if q.is_empty() => break,

                    _ => {
                        while let Some(n) = q.pop() {
                            v[n].fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            }
        });

        scope.spawn(|_| {
            for i in 0..COUNT {
                if let Some(n) = q.force_push(i) {
                    v[n].fetch_add(1, Ordering::SeqCst);
                }
            }

            t.fetch_sub(1, Ordering::SeqCst);
        });
    })
    .unwrap();

    for c in v {
        assert_eq!(c.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn mpmc() {
    check_mpmc(ArrayQueue::new(3));
}

#[test]
fn fallible_mpmc() {
    check_mpmc(ArrayQueue::try_new(3).unwrap());
}

fn check_mpmc(q: ArrayQueue<usize>) {
    const COUNT: usize = if cfg!(miri) { 50 } else { 25_000 };
    const THREADS: usize = 4;
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
                    while q.push(i).is_err() {}
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
fn mpmc_ring_buffer() {
    const COUNT: usize = if cfg!(miri) { 50 } else { 25_000 };
    const THREADS: usize = 4;

    let t = AtomicUsize::new(THREADS);
    let q = ArrayQueue::<usize>::new(3);
    let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

    scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                loop {
                    match t.load(Ordering::SeqCst) {
                        0 if q.is_empty() => break,

                        _ => {
                            while let Some(n) = q.pop() {
                                v[n].fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                }
            });
        }

        for _ in 0..THREADS {
            scope.spawn(|_| {
                for i in 0..COUNT {
                    if let Some(n) = q.force_push(i) {
                        v[n].fetch_add(1, Ordering::SeqCst);
                    }
                }

                t.fetch_sub(1, Ordering::SeqCst);
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
    let runs: usize = if cfg!(miri) { 3 } else { 100 };
    let steps: usize = if cfg!(miri) { 50 } else { 10_000 };
    let additional: usize = if cfg!(miri) { 10 } else { 50 };

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
        let q = ArrayQueue::new(50);

        scope(|scope| {
            scope.spawn(|_| {
                for _ in 0..steps {
                    while q.pop().is_none() {}
                }
            });

            scope.spawn(|_| {
                for _ in 0..steps {
                    while q.push(DropCounter).is_err() {
                        DROPS.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            });
        })
        .unwrap();

        for _ in 0..additional {
            q.push(DropCounter).unwrap();
        }

        assert_eq!(DROPS.load(Ordering::SeqCst), steps);
        drop(q);
        assert_eq!(DROPS.load(Ordering::SeqCst), steps + additional);
    }
}

#[test]
fn linearizable() {
    const COUNT: usize = if cfg!(miri) { 100 } else { 25_000 };
    const THREADS: usize = 4;

    let q = ArrayQueue::new(THREADS);

    scope(|scope| {
        for _ in 0..THREADS / 2 {
            scope.spawn(|_| {
                for _ in 0..COUNT {
                    while q.push(0).is_err() {}
                    q.pop().unwrap();
                }
            });

            scope.spawn(|_| {
                for _ in 0..COUNT {
                    if q.force_push(0).is_none() {
                        q.pop().unwrap();
                    }
                }
            });
        }
    })
    .unwrap();
}

#[test]
fn into_iter() {
    let q = ArrayQueue::new(100);
    for i in 0..100 {
        q.push(i).unwrap();
    }
    for (i, j) in q.into_iter().enumerate() {
        assert_eq!(i, j);
    }
}
