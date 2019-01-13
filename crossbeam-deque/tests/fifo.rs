extern crate crossbeam_deque as deque;
extern crate crossbeam_epoch as epoch;
extern crate rand;

use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::thread;

use deque::Steal::{Empty, Success};
use deque::Worker;
use rand::Rng;

#[test]
fn smoke() {
    let w = Worker::new_fifo();
    let s = w.stealer();
    assert_eq!(w.pop(), None);
    assert_eq!(s.steal(), Empty);

    w.push(1);
    assert_eq!(w.pop(), Some(1));
    assert_eq!(w.pop(), None);
    assert_eq!(s.steal(), Empty);

    w.push(2);
    assert_eq!(s.steal(), Success(2));
    assert_eq!(s.steal(), Empty);
    assert_eq!(w.pop(), None);

    w.push(3);
    w.push(4);
    w.push(5);
    assert_eq!(s.steal(), Success(3));
    assert_eq!(s.steal(), Success(4));
    assert_eq!(s.steal(), Success(5));
    assert_eq!(s.steal(), Empty);

    w.push(6);
    w.push(7);
    w.push(8);
    w.push(9);
    assert_eq!(w.pop(), Some(6));
    assert_eq!(s.steal(), Success(7));
    assert_eq!(w.pop(), Some(8));
    assert_eq!(w.pop(), Some(9));
    assert_eq!(w.pop(), None);
}

#[test]
fn steal_push() {
    const STEPS: usize = 50_000;

    let w = Worker::new_fifo();
    let s = w.stealer();
    let t = thread::spawn(move || {
        for i in 0..STEPS {
            loop {
                if let Success(v) = s.steal() {
                    assert_eq!(i, v);
                    break;
                }
            }
        }
    });

    for i in 0..STEPS {
        w.push(i);
    }
    t.join().unwrap();
}

#[test]
fn stampede() {
    const THREADS: usize = 8;
    const COUNT: usize = 50_000;

    let w = Worker::new_fifo();

    for i in 0..COUNT {
        w.push(Box::new(i + 1));
    }
    let remaining = Arc::new(AtomicUsize::new(COUNT));

    let threads = (0..THREADS)
        .map(|_| {
            let s = w.stealer();
            let remaining = remaining.clone();

            thread::spawn(move || {
                let mut last = 0;
                while remaining.load(SeqCst) > 0 {
                    if let Success(x) = s.steal() {
                        assert!(last < *x);
                        last = *x;
                        remaining.fetch_sub(1, SeqCst);
                    }
                }
            })
        }).collect::<Vec<_>>();

    let mut last = 0;
    while remaining.load(SeqCst) > 0 {
        if let Some(x) = w.pop() {
            assert!(last < *x);
            last = *x;
            remaining.fetch_sub(1, SeqCst);
        }
    }

    for t in threads {
        t.join().unwrap();
    }
}

fn run_stress() {
    const THREADS: usize = 8;
    const COUNT: usize = 50_000;

    let w = Worker::new_fifo();
    let done = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));

    let threads = (0..THREADS)
        .map(|_| {
            let s = w.stealer();
            let done = done.clone();
            let hits = hits.clone();

            thread::spawn(move || {
                let w2 = Worker::new_fifo();

                while !done.load(SeqCst) {
                    if let Success(_) = s.steal() {
                        hits.fetch_add(1, SeqCst);
                    }

                    if let Success(_) = s.steal_batch_and_pop(&w2) {
                        hits.fetch_add(1, SeqCst);

                        while let Some(_) = w2.pop() {
                            hits.fetch_add(1, SeqCst);
                        }
                    }
                }
            })
        }).collect::<Vec<_>>();

    let mut rng = rand::thread_rng();
    let mut expected = 0;
    while expected < COUNT {
        if rng.gen_range(0, 3) == 0 {
            while let Some(_) = w.pop() {
                hits.fetch_add(1, SeqCst);
            }
        } else {
            w.push(expected);
            expected += 1;
        }
    }

    while hits.load(SeqCst) < COUNT {
        while let Some(_) = w.pop() {
            hits.fetch_add(1, SeqCst);
        }
    }
    done.store(true, SeqCst);

    for t in threads {
        t.join().unwrap();
    }
}

#[test]
fn stress() {
    run_stress();
}

#[test]
fn stress_pinned() {
    let _guard = epoch::pin();
    run_stress();
}

#[test]
fn no_starvation() {
    const THREADS: usize = 8;
    const COUNT: usize = 50_000;

    let w = Worker::new_fifo();
    let done = Arc::new(AtomicBool::new(false));

    let (threads, hits): (Vec<_>, Vec<_>) = (0..THREADS)
        .map(|_| {
            let s = w.stealer();
            let done = done.clone();
            let hits = Arc::new(AtomicUsize::new(0));

            let t = {
                let hits = hits.clone();
                thread::spawn(move || {
                    let w2 = Worker::new_fifo();

                    while !done.load(SeqCst) {
                        if let Success(_) = s.steal() {
                            hits.fetch_add(1, SeqCst);
                        }

                        if let Success(_) = s.steal_batch_and_pop(&w2) {
                            hits.fetch_add(1, SeqCst);

                            while let Some(_) = w2.pop() {
                                hits.fetch_add(1, SeqCst);
                            }
                        }
                    }
                })
            };

            (t, hits)
        }).unzip();

    let mut rng = rand::thread_rng();
    let mut my_hits = 0;
    loop {
        for i in 0..rng.gen_range(0, COUNT) {
            if rng.gen_range(0, 3) == 0 && my_hits == 0 {
                while let Some(_) = w.pop() {
                    my_hits += 1;
                }
            } else {
                w.push(i);
            }
        }

        if my_hits > 0 && hits.iter().all(|h| h.load(SeqCst) > 0) {
            break;
        }
    }
    done.store(true, SeqCst);

    for t in threads {
        t.join().unwrap();
    }
}

#[test]
fn destructors() {
    const THREADS: usize = 8;
    const COUNT: usize = 50_000;
    const STEPS: usize = 1000;

    struct Elem(usize, Arc<Mutex<Vec<usize>>>);

    impl Drop for Elem {
        fn drop(&mut self) {
            self.1.lock().unwrap().push(self.0);
        }
    }

    let w = Worker::new_fifo();
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let remaining = Arc::new(AtomicUsize::new(COUNT));

    for i in 0..COUNT {
        w.push(Elem(i, dropped.clone()));
    }

    let threads = (0..THREADS)
        .map(|_| {
            let remaining = remaining.clone();
            let s = w.stealer();

            thread::spawn(move || {
                let w2 = Worker::new_fifo();
                let mut cnt = 0;

                while cnt < STEPS {
                    if let Success(_) = s.steal() {
                        cnt += 1;
                        remaining.fetch_sub(1, SeqCst);
                    }

                    if let Success(_) = s.steal_batch_and_pop(&w2) {
                        cnt += 1;
                        remaining.fetch_sub(1, SeqCst);

                        while let Some(_) = w2.pop() {
                            cnt += 1;
                            remaining.fetch_sub(1, SeqCst);
                        }
                    }
                }
            })
        }).collect::<Vec<_>>();

    for _ in 0..STEPS {
        if let Some(_) = w.pop() {
            remaining.fetch_sub(1, SeqCst);
        }
    }

    for t in threads {
        t.join().unwrap();
    }

    let rem = remaining.load(SeqCst);
    assert!(rem > 0);

    {
        let mut v = dropped.lock().unwrap();
        assert_eq!(v.len(), COUNT - rem);
        v.clear();
    }

    drop(w);

    {
        let mut v = dropped.lock().unwrap();
        assert_eq!(v.len(), rem);
        v.sort();
        for pair in v.windows(2) {
            assert_eq!(pair[0] + 1, pair[1]);
        }
    }
}
