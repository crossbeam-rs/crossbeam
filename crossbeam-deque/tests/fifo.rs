extern crate crossbeam_deque as deque;
extern crate crossbeam_epoch as epoch;
extern crate rand;

use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::thread;

use deque::Racy::{Done, Retry};
use rand::Rng;

#[test]
fn smoke() {
    let (w, s) = deque::fifo::<i32>();
    assert_eq!(w.pop(), Done(None));
    assert_eq!(s.steal_one(), Done(None));

    w.push(1);
    assert_eq!(w.pop(), Done(Some(1)));
    assert_eq!(w.pop(), Done(None));
    assert_eq!(s.steal_one(), Done(None));

    w.push(2);
    assert_eq!(s.steal_one(), Done(Some(2)));
    assert_eq!(s.steal_one(), Done(None));
    assert_eq!(w.pop(), Done(None));

    w.push(3);
    w.push(4);
    w.push(5);
    assert_eq!(s.steal_one(), Done(Some(3)));
    assert_eq!(s.steal_one(), Done(Some(4)));
    assert_eq!(s.steal_one(), Done(Some(5)));
    assert_eq!(s.steal_one(), Done(None));

    w.push(6);
    w.push(7);
    w.push(8);
    w.push(9);
    assert_eq!(w.pop(), Done(Some(6)));
    assert_eq!(s.steal_one(), Done(Some(7)));
    assert_eq!(w.pop(), Done(Some(8)));
    assert_eq!(w.pop(), Done(Some(9)));
    assert_eq!(w.pop(), Done(None));
}

#[test]
fn steal_push() {
    const STEPS: usize = 50_000;

    let (w, s) = deque::fifo();
    let t = thread::spawn(move || {
        for i in 0..STEPS {
            loop {
                if let Done(Some(v)) = s.steal_one() {
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

    let (w, s) = deque::fifo();

    for i in 0..COUNT {
        w.push(Box::new(i + 1));
    }
    let remaining = Arc::new(AtomicUsize::new(COUNT));

    let threads = (0..THREADS)
        .map(|_| {
            let s = s.clone();
            let remaining = remaining.clone();

            thread::spawn(move || {
                let mut last = 0;
                while remaining.load(SeqCst) > 0 {
                    if let Done(Some(x)) = s.steal_one() {
                        assert!(last < *x);
                        last = *x;
                        remaining.fetch_sub(1, SeqCst);
                    }
                }
            })
        }).collect::<Vec<_>>();

    let mut last = 0;
    while remaining.load(SeqCst) > 0 {
        loop {
            match w.pop() {
                Done(Some(x)) => {
                    assert!(last < *x);
                    last = *x;
                    remaining.fetch_sub(1, SeqCst);
                    break;
                }
                Done(None) => break,
                Retry => {}
            }
        }
    }

    for t in threads {
        t.join().unwrap();
    }
}

fn run_stress() {
    const THREADS: usize = 8;
    const COUNT: usize = 50_000;

    let (w, s) = deque::fifo();
    let done = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));

    let threads = (0..THREADS)
        .map(|_| {
            let s = s.clone();
            let done = done.clone();
            let hits = hits.clone();

            thread::spawn(move || {
                let (w2, _) = deque::fifo();

                while !done.load(SeqCst) {
                    if let Done(Some(_)) = s.steal_one() {
                        hits.fetch_add(1, SeqCst);
                    }

                    if let Done(Some(_)) = s.steal_one_and_batch(&w2) {
                        hits.fetch_add(1, SeqCst);

                        loop {
                            match w2.pop() {
                                Done(Some(_)) => {
                                    hits.fetch_add(1, SeqCst);
                                }
                                Done(None) => break,
                                Retry => {}
                            }
                        }
                    }
                }
            })
        }).collect::<Vec<_>>();

    let mut rng = rand::thread_rng();
    let mut expected = 0;
    while expected < COUNT {
        if rng.gen_range(0, 3) == 0 {
            loop {
                match w.pop() {
                    Done(Some(_)) => {
                        hits.fetch_add(1, SeqCst);
                    }
                    Done(None) => break,
                    Retry => {}
                }
            }
        } else {
            w.push(expected);
            expected += 1;
        }
    }

    while hits.load(SeqCst) < COUNT {
        loop {
            match w.pop() {
                Done(Some(_)) => {
                    hits.fetch_add(1, SeqCst);
                }
                Done(None) => break,
                Retry => {}
            }
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

    let (w, s) = deque::fifo();
    let done = Arc::new(AtomicBool::new(false));

    let (threads, hits): (Vec<_>, Vec<_>) = (0..THREADS)
        .map(|_| {
            let s = s.clone();
            let done = done.clone();
            let hits = Arc::new(AtomicUsize::new(0));

            let t = {
                let hits = hits.clone();
                thread::spawn(move || {
                    let (w2, _) = deque::fifo();

                    while !done.load(SeqCst) {
                        if let Done(Some(_)) = s.steal_one() {
                            hits.fetch_add(1, SeqCst);
                        }

                        if let Done(Some(_)) = s.steal_one_and_batch(&w2) {
                            hits.fetch_add(1, SeqCst);

                            loop {
                                match w2.pop() {
                                    Done(Some(_)) => {
                                        hits.fetch_add(1, SeqCst);
                                    }
                                    Done(None) => break,
                                    Retry => {}
                                }
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
                loop {
                    match w.pop() {
                        Done(Some(_)) => my_hits += 1,
                        Done(None) => break,
                        Retry => {}
                    }
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

    let (w, s) = deque::fifo();

    let dropped = Arc::new(Mutex::new(Vec::new()));
    let remaining = Arc::new(AtomicUsize::new(COUNT));
    for i in 0..COUNT {
        w.push(Elem(i, dropped.clone()));
    }

    let threads = (0..THREADS)
        .map(|_| {
            let remaining = remaining.clone();
            let s = s.clone();

            thread::spawn(move || {
                let (w2, _) = deque::fifo();
                let mut cnt = 0;

                while cnt < STEPS {
                    if let Done(Some(_)) = s.steal_one() {
                        cnt += 1;
                        remaining.fetch_sub(1, SeqCst);
                    }

                    if let Done(Some(_)) = s.steal_one_and_batch(&w2) {
                        cnt += 1;
                        remaining.fetch_sub(1, SeqCst);

                        loop {
                            match w2.pop() {
                                Done(Some(_)) => {
                                    cnt += 1;
                                    remaining.fetch_sub(1, SeqCst);
                                }
                                Done(None) => break,
                                Retry => {}
                            }
                        }
                    }
                }
            })
        }).collect::<Vec<_>>();

    for _ in 0..STEPS {
        loop {
            match w.pop() {
                Done(Some(_)) => {
                    remaining.fetch_sub(1, SeqCst);
                    break;
                }
                Done(None) => break,
                Retry => {}
            }
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

    drop((w, s));

    {
        let mut v = dropped.lock().unwrap();
        assert_eq!(v.len(), rem);
        v.sort();
        for pair in v.windows(2) {
            assert_eq!(pair[0] + 1, pair[1]);
        }
    }
}
