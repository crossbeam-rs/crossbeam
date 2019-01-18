extern crate crossbeam;

use crossbeam::queue::SegQueue;
use crossbeam::scope;

const CONC_COUNT: i64 = 1000000;

#[test]
fn push_pop_1() {
    let q: SegQueue<i64> = SegQueue::new();
    q.push(37);
    assert_eq!(q.try_pop(), Some(37));
}

#[test]
fn push_pop_2() {
    let q: SegQueue<i64> = SegQueue::new();
    q.push(37);
    q.push(48);
    assert_eq!(q.try_pop(), Some(37));
    assert_eq!(q.try_pop(), Some(48));
}

#[test]
fn push_pop_empty_check() {
    let q: SegQueue<i64> = SegQueue::new();
    assert_eq!(q.is_empty(), true);
    q.push(42);
    assert_eq!(q.is_empty(), false);
    assert_eq!(q.try_pop(), Some(42));
    assert_eq!(q.is_empty(), true);
}

#[test]
fn push_pop_many_seq() {
    let q: SegQueue<i64> = SegQueue::new();
    for i in 0..200 {
        q.push(i)
    }
    for i in 0..200 {
        assert_eq!(q.try_pop(), Some(i));
    }
}

#[test]
fn push_pop_many_spsc() {
    let q: SegQueue<i64> = SegQueue::new();

    scope(|scope| {
        scope.spawn(|_| {
            let mut next = 0;

            while next < CONC_COUNT {
                if let Some(elem) = q.try_pop() {
                    assert_eq!(elem, next);
                    next += 1;
                }
            }
        });

        for i in 0..CONC_COUNT {
            q.push(i)
        }
    }).unwrap();
}

#[test]
fn push_pop_many_spmc() {
    fn recv(_t: i32, q: &SegQueue<i64>) {
        let mut cur = -1;
        for _i in 0..CONC_COUNT {
            if let Some(elem) = q.try_pop() {
                assert!(elem > cur);
                cur = elem;

                if cur == CONC_COUNT - 1 {
                    break;
                }
            }
        }
    }

    let q: SegQueue<i64> = SegQueue::new();
    scope(|scope| {
        for i in 0..3 {
            let q = &q;
            scope.spawn(move |_| recv(i, q));
        }

        scope.spawn(|_| {
            for i in 0..CONC_COUNT {
                q.push(i);
            }
        });
    }).unwrap();
}

#[test]
fn push_pop_many_mpmc() {
    enum LR {
        Left(i64),
        Right(i64),
    }

    let q: SegQueue<LR> = SegQueue::new();

    scope(|scope| {
        for _t in 0..2 {
            scope.spawn(|_| {
                for i in CONC_COUNT - 1..CONC_COUNT {
                    q.push(LR::Left(i))
                }
            });
            scope.spawn(|_| {
                for i in CONC_COUNT - 1..CONC_COUNT {
                    q.push(LR::Right(i))
                }
            });
            scope.spawn(|_| {
                let mut vl = vec![];
                let mut vr = vec![];
                for _i in 0..CONC_COUNT {
                    match q.try_pop() {
                        Some(LR::Left(x)) => vl.push(x),
                        Some(LR::Right(x)) => vr.push(x),
                        _ => {}
                    }
                }

                let mut vl2 = vl.clone();
                let mut vr2 = vr.clone();
                vl2.sort();
                vr2.sort();

                assert_eq!(vl, vl2);
                assert_eq!(vr, vr2);
            });
        }
    }).unwrap();
}
