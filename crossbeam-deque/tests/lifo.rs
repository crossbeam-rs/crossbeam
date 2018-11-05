extern crate crossbeam_deque as deque;
extern crate crossbeam_epoch as epoch;
extern crate rand;

use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::thread;

use deque::{Pop, Steal};
use rand::Rng;

#[test]
fn smoke() {
    let (w, s) = deque::lifo::<i32>();
    assert_eq!(w.pop(), Pop::Empty);
    assert_eq!(s.steal(), Steal::Empty);

    w.push(1);
    assert_eq!(w.pop(), Pop::Data(1));
    assert_eq!(w.pop(), Pop::Empty);
    assert_eq!(s.steal(), Steal::Empty);

    w.push(2);
    assert_eq!(s.steal(), Steal::Data(2));
    assert_eq!(s.steal(), Steal::Empty);
    assert_eq!(w.pop(), Pop::Empty);

    w.push(3);
    w.push(4);
    w.push(5);
    assert_eq!(s.steal(), Steal::Data(3));
    assert_eq!(s.steal(), Steal::Data(4));
    assert_eq!(s.steal(), Steal::Data(5));
    assert_eq!(s.steal(), Steal::Empty);

    w.push(6);
    w.push(7);
    w.push(8);
    w.push(9);
    assert_eq!(w.pop(), Pop::Data(9));
    assert_eq!(s.steal(), Steal::Data(6));
    assert_eq!(w.pop(), Pop::Data(8));
    assert_eq!(w.pop(), Pop::Data(7));
    assert_eq!(w.pop(), Pop::Empty);
}

#[test]
fn steal_push() {
    const STEPS: usize = 50_000;

    let (w, s) = deque::lifo();
    let t = thread::spawn(move || {
        for i in 0..STEPS {
            loop {
                if let Steal::Data(v) = s.steal() {
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

    let (w, s) = deque::lifo();

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
                    if let Steal::Data(x) = s.steal() {
                        assert!(last < *x);
                        last = *x;
                        remaining.fetch_sub(1, SeqCst);
                    }
                }
            })
        }).collect::<Vec<_>>();

    let mut last = COUNT + 1;
    while remaining.load(SeqCst) > 0 {
        loop {
            match w.pop() {
                Pop::Data(x) => {
                    assert!(last > *x);
                    last = *x;
                    remaining.fetch_sub(1, SeqCst);
                    break;
                }
                Pop::Empty => break,
                Pop::Retry => {}
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

    let (w, s) = deque::lifo();
    let done = Arc::new(AtomicBool::new(false));
    let hits = Arc::new(AtomicUsize::new(0));

    let threads = (0..THREADS)
        .map(|_| {
            let s = s.clone();
            let done = done.clone();
            let hits = hits.clone();

            thread::spawn(move || {
                let (w2, _) = deque::lifo();

                while !done.load(SeqCst) {
                    if let Steal::Data(_) = s.steal() {
                        hits.fetch_add(1, SeqCst);
                    }

                    if let Steal::Data(_) = s.steal_many(&w2) {
                        hits.fetch_add(1, SeqCst);

                        loop {
                            match w2.pop() {
                                Pop::Data(_) => {
                                    hits.fetch_add(1, SeqCst);
                                }
                                Pop::Empty => break,
                                Pop::Retry => {}
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
                    Pop::Data(_) => {
                        hits.fetch_add(1, SeqCst);
                    }
                    Pop::Empty => break,
                    Pop::Retry => {}
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
                Pop::Data(_) => {
                    hits.fetch_add(1, SeqCst);
                }
                Pop::Empty => break,
                Pop::Retry => {}
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

    let (w, s) = deque::lifo();
    let done = Arc::new(AtomicBool::new(false));

    let (threads, hits): (Vec<_>, Vec<_>) = (0..THREADS)
        .map(|_| {
            let s = s.clone();
            let done = done.clone();
            let hits = Arc::new(AtomicUsize::new(0));

            let t = {
                let hits = hits.clone();
                thread::spawn(move || {
                    let (w2, _) = deque::lifo();

                    while !done.load(SeqCst) {
                        if let Steal::Data(_) = s.steal() {
                            hits.fetch_add(1, SeqCst);
                        }

                        if let Steal::Data(_) = s.steal_many(&w2) {
                            hits.fetch_add(1, SeqCst);

                            loop {
                                match w2.pop() {
                                    Pop::Data(_) => {
                                        hits.fetch_add(1, SeqCst);
                                    }
                                    Pop::Empty => break,
                                    Pop::Retry => {}
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
                        Pop::Data(_) => my_hits += 1,
                        Pop::Empty => break,
                        Pop::Retry => {}
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

    let (w, s) = deque::lifo();

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
                let (w2, _) = deque::lifo();
                let mut cnt = 0;

                while cnt < STEPS {
                    if let Steal::Data(_) = s.steal() {
                        cnt += 1;
                        remaining.fetch_sub(1, SeqCst);
                    }

                    if let Steal::Data(_) = s.steal_many(&w2) {
                        cnt += 1;
                        remaining.fetch_sub(1, SeqCst);

                        loop {
                            match w2.pop() {
                                Pop::Data(_) => {
                                    cnt += 1;
                                    remaining.fetch_sub(1, SeqCst);
                                }
                                Pop::Empty => break,
                                Pop::Retry => {}
                            }
                        }
                    }
                }
            })
        }).collect::<Vec<_>>();

    for _ in 0..STEPS {
        loop {
            match w.pop() {
                Pop::Data(_) => {
                    remaining.fetch_sub(1, SeqCst);
                    break;
                }
                Pop::Empty => break,
                Pop::Retry => {}
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
