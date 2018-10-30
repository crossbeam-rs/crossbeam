//! Tests for the `Select` struct.

extern crate crossbeam;
extern crate crossbeam_channel as channel;

use std::any::Any;
use std::cell::Cell;
use std::thread;
use std::time::{Duration, Instant};

use channel::Select;
use channel::TryRecvError;

// TODO: If SelectedCase is dropped without being completed, panic!
// TODO: test: verify that Select can be reused
// TODO: move most of the stuff inside `internal` into the parent module?
// TODO: verify that compile_error! and unreachable! work in edition 2018
// TODO: disconnection vs closing? probably disconnection
// TODO: try_select and select_timeout should return Result<SelectedCase, TrySelectError/SelectTimeoutError>

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke1() {
    let (s1, r1) = channel::unbounded::<usize>();
    let (s2, r2) = channel::unbounded::<usize>();

    s1.send(1).unwrap();

    let mut sel = Select::new();
    let case1 = sel.recv(&r1);
    let case2 = sel.recv(&r2);
    let case = sel.select();
    match case.index() {
        i if i == case1 => assert_eq!(case.recv(&r1), Ok(1)),
        i if i == case2 => panic!(),
        _ => unreachable!(),
    }

    s2.send(2).unwrap();

    let mut sel = Select::new();
    let case1 = sel.recv(&r1);
    let case2 = sel.recv(&r2);
    let case = sel.select();
    match case.index() {
        i if i == case1 => panic!(),
        i if i == case2 => assert_eq!(case.recv(&r2), Ok(2)),
        _ => unreachable!(),
    }
}

#[test]
fn smoke2() {
    let (_s1, r1) = channel::unbounded::<i32>();
    let (_s2, r2) = channel::unbounded::<i32>();
    let (_s3, r3) = channel::unbounded::<i32>();
    let (_s4, r4) = channel::unbounded::<i32>();
    let (s5, r5) = channel::unbounded::<i32>();

    s5.send(5).unwrap();

    let mut sel = Select::new();
    let case1 = sel.recv(&r1);
    let case2 = sel.recv(&r2);
    let case3 = sel.recv(&r3);
    let case4 = sel.recv(&r4);
    let case5 = sel.recv(&r5);
    let case = sel.select();
    match case.index() {
        i if i == case1 => panic!(),
        i if i == case2 => panic!(),
        i if i == case3 => panic!(),
        i if i == case4 => panic!(),
        i if i == case5 => assert_eq!(case.recv(&r5), Ok(5)),
        _ => unreachable!(),
    }
}

#[test]
fn closed() {
    let (s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            drop(s1);
            thread::sleep(ms(500));
            s2.send(5).unwrap();
        });

        let mut sel = Select::new();
        let case1 = sel.recv(&r1);
        let case2 = sel.recv(&r2);
        match sel.select_timeout(ms(1000)) {
            None => panic!(),
            Some(case) => match case.index() {
                i if i == case1 => assert!(case.recv(&r1).is_err()),
                i if i == case2 => panic!(),
                _ => unreachable!(),
            }
        }

        r2.recv().unwrap();
    });

    let mut sel = Select::new();
    let case1 = sel.recv(&r1);
    let case2 = sel.recv(&r2);
    match sel.select_timeout(ms(1000)) {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert!(case.recv(&r1).is_err()),
            i if i == case2 => panic!(),
            _ => unreachable!(),
        }
    }

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            drop(s2);
        });

        let mut sel = Select::new();
        let case1 = sel.recv(&r2);
        match sel.select_timeout(ms(1000)) {
            None => panic!(),
            Some(case) => match case.index() {
                i if i == case1 => assert!(case.recv(&r2).is_err()),
                i if i == case2 => panic!(),
                _ => unreachable!(),
            }
        }
    });
}

#[test]
fn default() {
    let (s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();

    let mut sel = Select::new();
    let _case1 = sel.recv(&r1);
    let _case2 = sel.recv(&r2);
    match sel.try_select() {
        None => {}
        Some(_) => panic!(),
    }

    drop(s1);

    let mut sel = Select::new();
    let case1 = sel.recv(&r1);
    let case2 = sel.recv(&r2);
    match sel.try_select() {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert!(case.recv(&r1).is_err()),
            i if i == case2 => panic!(),
            _ => unreachable!()
        }
    }

    s2.send(2).unwrap();

    let mut sel = Select::new();
    let case1 = sel.recv(&r2);
    match sel.try_select() {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert_eq!(case.recv(&r2), Ok(2)),
            _ => unreachable!()
        }
    }

    let mut sel = Select::new();
    let _case1 = sel.recv(&r2);
    match sel.try_select() {
        None => {}
        Some(_) => panic!(),
    }

    let mut sel = Select::new();
    match sel.try_select() {
        None => {}
        Some(_) => panic!(),
    }
}

#[test]
fn timeout() {
    let (_s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(1500));
            s2.send(2).unwrap();
        });

        let mut sel = Select::new();
        let case1 = sel.recv(&r1);
        let case2 = sel.recv(&r2);
        match sel.select_timeout(ms(1000)) {
            None => {}
            Some(case) => match case.index() {
                i if i == case1 => panic!(),
                i if i == case2 => panic!(),
                _ => unreachable!(),
            }
        }

        let mut sel = Select::new();
        let case1 = sel.recv(&r1);
        let case2 = sel.recv(&r2);
        match sel.select_timeout(ms(1000)) {
            None => panic!(),
            Some(case) => match case.index() {
                i if i == case1 => panic!(),
                i if i == case2 => assert_eq!(case.recv(&r2), Ok(2)),
                _ => unreachable!(),
            }
        }
    });

    crossbeam::scope(|scope| {
        let (s, r) = channel::unbounded::<i32>();

        scope.spawn(move || {
            thread::sleep(ms(500));
            drop(s);
        });

        let mut sel = Select::new();
        match sel.select_timeout(ms(1000)) {
            None => {
                let mut sel = Select::new();
                let case1 = sel.recv(&r);
                match sel.try_select() {
                    None => panic!(),
                    Some(case) => match case.index() {
                        i if i == case1 => assert!(case.recv(&r).is_err()),
                        _ => unreachable!(),
                    }
                }
            }
            Some(_) => unreachable!(),
        }
    });
}

#[test]
fn default_when_closed() {
    let (_, r) = channel::unbounded::<i32>();

    let mut sel = Select::new();
    let case1 = sel.recv(&r);
    match sel.try_select() {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert!(case.recv(&r).is_err()),
            _ => unreachable!(),
        }
    }

    let (_, r) = channel::unbounded::<i32>();

    let mut sel = Select::new();
    let case1 = sel.recv(&r);
    match sel.select_timeout(ms(1000)) {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert!(case.recv(&r).is_err()),
            _ => unreachable!(),
        }
    }

    let (s, _) = channel::bounded::<i32>(0);

    let mut sel = Select::new();
    let case1 = sel.send(&s);
    match sel.try_select() {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert!(case.send(&s, 0).is_err()),
            _ => unreachable!(),
        }
    }

    let (s, _) = channel::bounded::<i32>(0);

    let mut sel = Select::new();
    let case1 = sel.send(&s);
    match sel.select_timeout(ms(1000)) {
        None => panic!(),
        Some(case) => match case.index() {
            i if i == case1 => assert!(case.send(&s, 0).is_err()),
            _ => unreachable!(),
        }
    }
}

#[test]
fn default_only() {
    let start = Instant::now();
    assert!(Select::new().try_select().is_none());
    let now = Instant::now();
    assert!(now - start <= ms(50));

    let start = Instant::now();
    assert!(Select::new().select_timeout(ms(500)).is_none());
    let now = Instant::now();
    assert!(now - start >= ms(450));
    assert!(now - start <= ms(550));
}


#[test]
fn unblocks() {
    let (s1, r1) = channel::bounded::<i32>(0);
    let (s2, r2) = channel::bounded::<i32>(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s2.send(2).unwrap();
        });

        let mut sel = Select::new();
        let case1 = sel.recv(&r1);
        let case2 = sel.recv(&r2);
        match sel.select_timeout(ms(1000)) {
            None => panic!(),
            Some(case) => match case.index() {
                i if i == case1 => panic!(),
                i if i == case2 => assert_eq!(case.recv(&r2), Ok(2)),
                _ => unreachable!(),
            }
        }
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            assert_eq!(r1.recv().unwrap(), 1);
        });

        let mut sel = Select::new();
        let case1 = sel.send(&s1);
        let case2 = sel.send(&s2);
        match sel.select_timeout(ms(1000)) {
            None => panic!(),
            Some(case) => match case.index() {
                i if i == case1 => case.send(&s1, 1).unwrap(),
                i if i == case2 => panic!(),
                _ => unreachable!(),
            }
        }
    });
}

#[test]
fn both_ready() {
    let (s1, r1) = channel::bounded(0);
    let (s2, r2) = channel::bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s1.send(1).unwrap();
            assert_eq!(r2.recv().unwrap(), 2);
        });

        for _ in 0..2 {
            let mut sel = Select::new();
            let case1 = sel.recv(&r1);
            let case2 = sel.send(&s2);
            let case = sel.select();
            match case.index() {
                i if i == case1 => assert_eq!(case.recv(&r1), Ok(1)),
                i if i == case2 => case.send(&s2, 2).unwrap(),
                _ => unreachable!(),
            }
        }
    });
}

#[test]
fn loop_try() {
    const RUNS: usize = 20;

    for _ in 0..RUNS {
        let (s1, r1) = channel::bounded::<i32>(0);
        let (s2, r2) = channel::bounded::<i32>(0);
        let (s_end, r_end) = channel::bounded::<()>(0);

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                loop {
                    let mut done = false;

                    let mut sel = Select::new();
                    let case1 = sel.send(&s1);
                    match sel.try_select() {
                        None => {}
                        Some(case) => match case.index() {
                            i if i == case1 => {
                                let _ = case.send(&s1, 1);
                                done = true;
                            }
                            _ => unreachable!(),
                        }
                    }
                    if done {
                        break;
                    }

                    let mut sel = Select::new();
                    let case1 = sel.recv(&r_end);
                    match sel.try_select() {
                        None => {}
                        Some(case) => match case.index() {
                            i if i == case1 => {
                                let _ = case.recv(&r_end);
                                done = true;
                            }
                            _ => unreachable!(),
                        }
                    }
                    if done {
                        break;
                    }
                }
            });

            scope.spawn(|| {
                loop {
                    if let Ok(x) = r2.try_recv() {
                        assert_eq!(x, 2);
                        break;
                    }

                    let mut done = false;
                    let mut sel = Select::new();
                    let case1 = sel.recv(&r_end);
                    match sel.try_select() {
                        None => {}
                        Some(case) => match case.index() {
                            i if i == case1 => {
                                let _ = case.recv(&r_end);
                                done = true;
                            }
                            _ => unreachable!(),
                        }
                    }
                    if done {
                        break;
                    }
                }
            });

            scope.spawn(|| {
                thread::sleep(ms(500));

                let mut sel = Select::new();
                let case1 = sel.recv(&r1);
                let case2 = sel.send(&s2);
                match sel.select_timeout(ms(1000)) {
                    None => {}
                    Some(case) => match case.index() {
                        i if i == case1 => assert_eq!(case.recv(&r1), Ok(1)),
                        i if i == case2 => assert!(case.send(&s2, 2).is_ok()),
                        _ => unreachable!(),
                    }
                }

                drop(s_end);
            });
        });
    }
}

#[test]
fn cloning1() {
    crossbeam::scope(|scope| {
        let (s1, r1) = channel::unbounded::<i32>();
        let (_s2, r2) = channel::unbounded::<i32>();
        let (s3, r3) = channel::unbounded::<()>();

        scope.spawn(move || {
            r3.recv().unwrap();
            drop(s1.clone());
            assert!(r3.try_recv().is_err());
            s1.send(1).unwrap();
            r3.recv().unwrap();
        });

        s3.send(()).unwrap();

        let mut sel = Select::new();
        let case1 = sel.recv(&r1);
        let case2 = sel.recv(&r2);
        let case = sel.select();
        match case.index() {
            i if i == case1 => drop(case.recv(&r1)),
            i if i == case2 => drop(case.recv(&r2)),
            _ => unreachable!(),
        }

        s3.send(()).unwrap();
    });
}

#[test]
fn cloning2() {
    let (s1, r1) = channel::unbounded::<()>();
    let (s2, r2) = channel::unbounded::<()>();
    let (_s3, _r3) = channel::unbounded::<()>();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            let mut sel = Select::new();
            let case1 = sel.recv(&r1);
            let case2 = sel.recv(&r2);
            let case = sel.select();
            match case.index() {
                i if i == case1 => panic!(),
                i if i == case2 => drop(case.recv(&r2)),
                _ => unreachable!(),
            }
        });

        thread::sleep(ms(500));
        drop(s1.clone());
        s2.send(()).unwrap();
    })
}

#[test]
fn preflight1() {
    let (s, r) = channel::unbounded();
    s.send(()).unwrap();

    let mut sel = Select::new();
    let case1 = sel.recv(&r);
    let case = sel.select();
    match case.index() {
        i if i == case1 => drop(case.recv(&r)),
        _ => unreachable!(),
    }
}

#[test]
fn preflight2() {
    let (s, r) = channel::unbounded();
    drop(s.clone());
    s.send(()).unwrap();
    drop(s);

    let mut sel = Select::new();
    let case1 = sel.recv(&r);
    let case = sel.select();
    match case.index() {
        i if i == case1 => assert_eq!(case.recv(&r), Ok(())),
        _ => unreachable!(),
    }

    assert_eq!(r.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn preflight3() {
    let (s, r) = channel::unbounded();
    drop(s.clone());
    s.send(()).unwrap();
    drop(s);
    r.recv().unwrap();

    let mut sel = Select::new();
    let case1 = sel.recv(&r);
    let case = sel.select();
    match case.index() {
        i if i == case1 => assert!(case.recv(&r).is_err()),
        _ => unreachable!(),
    }
}

#[test]
fn duplicate_cases() {
    let (s, r) = channel::unbounded::<i32>();
    let hit = vec![Cell::new(false); 4];

    while hit.iter().map(|h| h.get()).any(|hit| !hit) {
        let mut sel = Select::new();
        let case0 = sel.recv(&r);
        let case1 = sel.recv(&r);
        let case2 = sel.send(&s);
        let case3 = sel.send(&s);
        let case = sel.select();
        match case.index() {
            i if i == case0 => {
                assert!(case.recv(&r).is_ok());
                hit[0].set(true);
            }
            i if i == case1 => {
                assert!(case.recv(&r).is_ok());
                hit[1].set(true);
            }
            i if i == case2 => {
                assert!(case.send(&s, 0).is_ok());
                hit[2].set(true);
            }
            i if i == case3 => {
                assert!(case.send(&s, 0).is_ok());
                hit[3].set(true);
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn nesting() {
    let (s, r) = channel::unbounded::<i32>();

    let mut sel = Select::new();
    let case1 = sel.send(&s);
    let case = sel.select();
    match case.index() {
        i if i == case1 => {
            assert!(case.send(&s, 0).is_ok());

            let mut sel = Select::new();
            let case1 = sel.recv(&r);
            let case = sel.select();
            match case.index() {
                i if i == case1 => {
                    assert_eq!(case.recv(&r), Ok(0));

                    let mut sel = Select::new();
                    let case1 = sel.send(&s);
                    let case = sel.select();
                    match case.index() {
                        i if i == case1 => {
                            assert!(case.send(&s, 1).is_ok());

                            let mut sel = Select::new();
                            let case1 = sel.recv(&r);
                            let case = sel.select();
                            match case.index() {
                                i if i == case1 => {
                                    assert_eq!(case.recv(&r), Ok(1));
                                }
                                _ => unreachable!(),
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

#[test]
fn stress_recv() {
    const COUNT: usize = 10_000;

    let (s1, r1) = channel::unbounded();
    let (s2, r2) = channel::bounded(5);
    let (s3, r3) = channel::bounded(100);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                s1.send(i).unwrap();
                r3.recv().unwrap();

                s2.send(i).unwrap();
                r3.recv().unwrap();
            }
        });

        for i in 0..COUNT {
            for _ in 0..2 {
                let mut sel = Select::new();
                let case1 = sel.recv(&r1);
                let case2 = sel.recv(&r2);
                let case = sel.select();
                match case.index() {
                    c if c == case1 => assert_eq!(case.recv(&r1), Ok(i)),
                    c if c == case2 => assert_eq!(case.recv(&r2), Ok(i)),
                    _ => unreachable!(),
                }

                s3.send(()).unwrap();
            }
        }
    });
}

#[test]
fn stress_send() {
    const COUNT: usize = 10_000;

    let (s1, r1) = channel::bounded(0);
    let (s2, r2) = channel::bounded(0);
    let (s3, r3) = channel::bounded(100);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                assert_eq!(r1.recv().unwrap(), i);
                assert_eq!(r2.recv().unwrap(), i);
                r3.recv().unwrap();
            }
        });

        for i in 0..COUNT {
            for _ in 0..2 {
                let mut sel = Select::new();
                let case1 = sel.send(&s1);
                let case2 = sel.send(&s2);
                let case = sel.select();
                match case.index() {
                    c if c == case1 => assert!(case.send(&s1, i).is_ok()),
                    c if c == case2 => assert!(case.send(&s2, i).is_ok()),
                    _ => unreachable!(),
                }
            }
            s3.send(()).unwrap();
        }
    });
}

#[test]
fn stress_mixed() {
    const COUNT: usize = 10_000;

    let (s1, r1) = channel::bounded(0);
    let (s2, r2) = channel::bounded(0);
    let (s3, r3) = channel::bounded(100);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                s1.send(i).unwrap();
                assert_eq!(r2.recv().unwrap(), i);
                r3.recv().unwrap();
            }
        });

        for i in 0..COUNT {
            for _ in 0..2 {
                let mut sel = Select::new();
                let case1 = sel.recv(&r1);
                let case2 = sel.send(&s2);
                let case = sel.select();
                match case.index() {
                    c if c == case1 => assert_eq!(case.recv(&r1), Ok(i)),
                    c if c == case2 => assert!(case.send(&s2, i).is_ok()),
                    _ => unreachable!(),
                }
            }
            s3.send(()).unwrap();
        }
    });
}

#[test]
fn stress_timeout_two_threads() {
    const COUNT: usize = 20;

    let (s, r) = channel::bounded(2);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(500));
                }

                let mut done = false;
                while !done {
                    let mut sel = Select::new();
                    let case1 = sel.send(&s);
                    match sel.select_timeout(ms(100)) {
                        None => {}
                        Some(case) => match case.index() {
                            c if c == case1 => {
                                assert!(case.send(&s, i).is_ok());
                                break;
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        });

        scope.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(500));
                }

                let mut done = false;
                while !done {
                    let mut sel = Select::new();
                    let case1 = sel.recv(&r);
                    match sel.select_timeout(ms(100)) {
                        None => {}
                        Some(case) => match case.index() {
                            c if c == case1 => {
                                assert_eq!(case.recv(&r), Ok(i));
                                done = true;
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        });
    });
}

#[test]
fn send_recv_same_channel() {
    let (s, r) = channel::bounded::<i32>(0);
    let mut sel = Select::new();
    let case1 = sel.send(&s);
    let case2 = sel.recv(&r);
    match sel.select_timeout(ms(100)) {
        None => {}
        Some(case) => match case.index() {
            c if c == case1 => panic!(),
            c if c == case2 => panic!(),
            _ => unreachable!(),
        }
    }

    let (s, r) = channel::unbounded::<i32>();
    let mut sel = Select::new();
    let case1 = sel.send(&s);
    let case2 = sel.recv(&r);
    match sel.select_timeout(ms(100)) {
        None => panic!(),
        Some(case) => match case.index() {
            c if c == case1 => assert!(case.send(&s, 0).is_ok()),
            c if c == case2 => panic!(),
            _ => unreachable!(),
        }
    }
}

#[test]
fn matching() {
    const THREADS: usize = 44;

    let (s, r) = &channel::bounded::<usize>(0);

    crossbeam::scope(|scope| {
        for i in 0..THREADS {
            scope.spawn(move || {
                let mut sel = Select::new();
                let case1 = sel.recv(&r);
                let case2 = sel.send(&s);
                let case = sel.select();
                match case.index() {
                    c if c == case1 => assert_ne!(case.recv(&r), Ok(i)),
                    c if c == case2 => assert!(case.send(&s, i).is_ok()),
                    _ => unreachable!(),
                }
            });
        }
    });

    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn matching_with_leftover() {
    const THREADS: usize = 55;

    let (s, r) = &channel::bounded::<usize>(0);

    crossbeam::scope(|scope| {
        for i in 0..THREADS {
            scope.spawn(move || {
                let mut sel = Select::new();
                let case1 = sel.recv(&r);
                let case2 = sel.send(&s);
                let case = sel.select();
                match case.index() {
                    c if c == case1 => assert_ne!(case.recv(&r), Ok(i)),
                    c if c == case2 => assert!(case.send(&s, i).is_ok()),
                    _ => unreachable!(),
                }
            });
        }
        s.send(!0).unwrap();
    });

    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn channel_through_channel() {
    const COUNT: usize = 1000;

    type T = Box<Any + Send>;

    for cap in 0..3 {
        let (s, r) = channel::bounded::<T>(cap);

        crossbeam::scope(|scope| {
            scope.spawn(move || {
                let mut s = s;

                for _ in 0..COUNT {
                    let (new_s, new_r) = channel::bounded(cap);
                    let mut new_r: T = Box::new(Some(new_r));

                    {
                        let mut sel = Select::new();
                        let case1 = sel.send(&s);
                        let case = sel.select();
                        match case.index() {
                            c if c == case1 => assert!(case.send(&s, new_r).is_ok()),
                            _ => unreachable!(),
                        }
                    }

                    s = new_s;
                }
            });

            scope.spawn(move || {
                let mut r = r;

                for _ in 0..COUNT {
                    let new = {
                        let mut sel = Select::new();
                        let case1 = sel.recv(&r);
                        let case = sel.select();
                        match case.index() {
                            c if c == case1 => {
                                 case.recv(&r)
                                    .unwrap()
                                    .downcast_mut::<Option<channel::Receiver<T>>>()
                                    .unwrap()
                                    .take()
                                    .unwrap()
                            }
                            _ => unreachable!(),
                        }
                    };
                    r = new;
                }
            });
        });
    }
}

#[test]
fn linearizable_try() {
    const COUNT: usize = 100_000;

    for step in 0..2 {
        let (start_s, start_r) = channel::bounded::<()>(0);
        let (end_s, end_r) = channel::bounded::<()>(0);

        let ((s1, r1), (s2, r2)) = if step == 0 {
            (channel::bounded::<i32>(1), channel::bounded::<i32>(1))
        } else {
            (channel::unbounded::<i32>(), channel::unbounded::<i32>())
        };

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..COUNT {
                    start_s.send(()).unwrap();

                    s1.send(1).unwrap();

                    let mut sel = Select::new();
                    let case1 = sel.recv(&r1);
                    let case2 = sel.recv(&r2);
                    match sel.try_select() {
                        None => unreachable!(),
                        Some(case) => match case.index() {
                            c if c == case1 => assert!(case.recv(&r1).is_ok()),
                            c if c == case2 => assert!(case.recv(&r2).is_ok()),
                            _ => unreachable!(),
                        }
                    }

                    end_s.send(()).unwrap();
                    let _ = r2.try_recv();
                }
            });

            for _ in 0..COUNT {
                start_r.recv().unwrap();

                s2.send(1).unwrap();
                let _ = r1.try_recv();

                end_r.recv().unwrap();
            }
        });
    }
}

#[test]
fn linearizable_timeout() {
    const COUNT: usize = 100_000;

    for step in 0..2 {
        let (start_s, start_r) = channel::bounded::<()>(0);
        let (end_s, end_r) = channel::bounded::<()>(0);

        let ((s1, r1), (s2, r2)) = if step == 0 {
            (channel::bounded::<i32>(1), channel::bounded::<i32>(1))
        } else {
            (channel::unbounded::<i32>(), channel::unbounded::<i32>())
        };

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..COUNT {
                    start_s.send(()).unwrap();

                    s1.send(1).unwrap();

                    let mut sel = Select::new();
                    let case1 = sel.recv(&r1);
                    let case2 = sel.recv(&r2);
                    match sel.select_timeout(ms(0)) {
                        None => unreachable!(),
                        Some(case) => match case.index() {
                            c if c == case1 => assert!(case.recv(&r1).is_ok()),
                            c if c == case2 => assert!(case.recv(&r2).is_ok()),
                            _ => unreachable!(),
                        }
                    }

                    end_s.send(()).unwrap();
                    let _ = r2.try_recv();
                }
            });

            for _ in 0..COUNT {
                start_r.recv().unwrap();

                s2.send(1).unwrap();
                let _ = r1.try_recv();

                end_r.recv().unwrap();
            }
        });
    }
}

#[test]
fn fairness1() {
    const COUNT: usize = 10_000;

    let (s1, r1) = channel::bounded::<()>(COUNT);
    let (s2, r2) = channel::unbounded::<()>();

    for _ in 0..COUNT {
        s1.send(()).unwrap();
        s2.send(()).unwrap();
    }

    let hits = vec![Cell::new(0usize); 4];
    while hits[0].get() + hits[1].get() < 2 * COUNT {
        let after = channel::after(ms(0));
        let tick = channel::tick(ms(0));

        let mut sel = Select::new();
        let case1 = sel.recv(&r1);
        let case2 = sel.recv(&r2);
        let case3 = sel.recv(&after);
        let case4 = sel.recv(&tick);
        let case = sel.select();
        match case.index() {
            i if i == case1 => {
                case.recv(&r1).unwrap();
                hits[0].set(hits[0].get() + 1);
            }
            i if i == case2 => {
                case.recv(&r2).unwrap();
                hits[1].set(hits[1].get() + 1);
            }
            i if i == case3 => {
                case.recv(&after).unwrap();
                hits[2].set(hits[2].get() + 1);
            }
            i if i == case4 => {
                case.recv(&tick).unwrap();
                hits[3].set(hits[3].get() + 1);
            }
            _ => unreachable!(),
        }
    }

    assert!(r1.is_empty());
    assert!(r2.is_empty());

    let sum: usize = hits.iter().map(|h| h.get()).sum();
    assert!(hits.iter().all(|x| x.get() >= sum / hits.len() / 2));
}

#[test]
fn fairness2() {
    const COUNT: usize = 10_000;

    let (s1, r1) = channel::unbounded::<()>();
    let (s2, r2) = channel::bounded::<()>(1);
    let (s3, r3) = channel::bounded::<()>(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..COUNT {
                let mut sel = Select::new();
                let mut case1 = None;
                let mut case2 = None;
                if s1.is_empty() {
                    case1 = Some(sel.send(&s1));
                }
                if s2.is_empty() {
                    case2 = Some(sel.send(&s2));
                }
                let case3 = sel.send(&s3);
                let case = sel.select();
                match case.index() {
                    i if Some(i) == case1 => assert!(case.send(&s1, ()).is_ok()),
                    i if Some(i) == case2 => assert!(case.send(&s2, ()).is_ok()),
                    i if i == case3 => assert!(case.send(&s3, ()).is_ok()),
                    _ => unreachable!(),
                }
            }
        });

        let hits = vec![Cell::new(0usize); 3];
        for _ in 0..COUNT {
            let mut sel = Select::new();
            let case1 = sel.recv(&r1);
            let case2 = sel.recv(&r2);
            let case3 = sel.recv(&r3);
            let case = sel.select();
            match case.index() {
                i if i == case1 => {
                    case.recv(&r1).unwrap();
                    hits[0].set(hits[0].get() + 1);
                }
                i if i == case2 => {
                    case.recv(&r2).unwrap();
                    hits[1].set(hits[1].get() + 1);
                }
                i if i == case3 => {
                    case.recv(&r3).unwrap();
                    hits[2].set(hits[2].get() + 1);
                }
                _ => unreachable!(),
            }
        }
        assert!(hits.iter().all(|x| x.get() >= COUNT / hits.len() / 10));
    });
}
