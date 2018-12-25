extern crate crossbeam_circbuf as circbuf;
extern crate crossbeam_utils as utils;
extern crate rand;

use std::sync::atomic::{AtomicUsize, Ordering};

use circbuf::mpmc::bounded::Queue;
use rand::{thread_rng, Rng};

#[test]
fn smoke() {
    let q = Queue::new(1);

    q.send(7).unwrap();
    assert_eq!(q.recv(), Some(7));

    q.send(8).unwrap();
    assert_eq!(q.recv(), Some(8));
    assert_eq!(q.recv(), None);
}

#[test]
fn cap() {
    for i in 1..10 {
        let q = Queue::<i32>::new(i);
        assert_eq!(q.cap(), i);
    }
}

#[test]
fn len_empty_full() {
    let q = Queue::new(2);

    assert_eq!(q.len(), 0);
    assert_eq!(q.is_empty(), true);
    assert_eq!(q.is_full(), false);

    q.send(()).unwrap();

    assert_eq!(q.len(), 1);
    assert_eq!(q.is_empty(), false);
    assert_eq!(q.is_full(), false);

    q.send(()).unwrap();

    assert_eq!(q.len(), 2);
    assert_eq!(q.is_empty(), false);
    assert_eq!(q.is_full(), true);

    println!("before: {:?}", q);
    q.recv().unwrap();
    println!("before: {:?}", q);

    assert_eq!(q.len(), 1);
    assert_eq!(q.is_empty(), false);
    assert_eq!(q.is_full(), false);
}

#[test]
fn len() {
    const COUNT: usize = 25_000;
    const CAP: usize = 1000;

    let q = Queue::new(CAP);
    assert_eq!(q.len(), 0);

    for _ in 0..CAP / 10 {
        for i in 0..50 {
            q.send(i).unwrap();
            assert_eq!(q.len(), i + 1);
        }

        for i in 0..50 {
            q.recv().unwrap();
            assert_eq!(q.len(), 50 - i - 1);
        }
    }
    assert_eq!(q.len(), 0);

    for i in 0..CAP {
        q.send(i).unwrap();
        assert_eq!(q.len(), i + 1);
    }

    for _ in 0..CAP {
        q.recv().unwrap();
    }
    assert_eq!(q.len(), 0);

    utils::thread::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..COUNT {
                loop {
                    if let Some(x) = q.recv() {
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
                while q.send(i).is_err() {}
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
    const COUNT: usize = 100_000;

    let q = Queue::new(3);

    utils::thread::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..COUNT {
                loop {
                    if let Some(x) = q.recv() {
                        assert_eq!(x, i);
                        break;
                    }
                }
            }
            assert_eq!(q.recv(), None);
        });

        scope.spawn(|_| {
            for i in 0..COUNT {
                while q.send(i).is_err() {}
            }
        });
    })
    .unwrap();
}

#[test]
fn mpmc() {
    const COUNT: usize = 25_000;
    const THREADS: usize = 4;

    let q = Queue::<usize>::new(3);
    let v = (0..COUNT).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();

    utils::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..COUNT {
                    let n = loop {
                        if let Some(x) = q.recv() {
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
                    while q.send(i).is_err() {}
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
    const RUNS: usize = 100;

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug, PartialEq)]
    struct DropCounter;

    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    let mut rng = thread_rng();

    for _ in 0..RUNS {
        let steps = rng.gen_range(0, 10_000);
        let additional = rng.gen_range(0, 50);

        DROPS.store(0, Ordering::SeqCst);
        let q = Queue::new(50);

        utils::thread::scope(|scope| {
            scope.spawn(|_| {
                for _ in 0..steps {
                    while q.recv().is_none() {}
                }
            });

            scope.spawn(|_| {
                for _ in 0..steps {
                    while q.send(DropCounter).is_err() {
                        DROPS.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            });
        })
        .unwrap();

        for _ in 0..additional {
            q.send(DropCounter).unwrap();
        }

        assert_eq!(DROPS.load(Ordering::SeqCst), steps);
        drop(q);
        assert_eq!(DROPS.load(Ordering::SeqCst), steps + additional);
    }
}

#[test]
fn linearizable() {
    const COUNT: usize = 25_000;
    const THREADS: usize = 4;

    let q = Queue::new(THREADS);

    utils::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                for _ in 0..COUNT {
                    while q.send(0).is_err() {}
                    q.recv().unwrap();
                }
            });
        }
    })
    .unwrap();
}
