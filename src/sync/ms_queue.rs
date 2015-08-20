use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::cell::RefCell;

use mem::epoch::{self, AtomicPtr, Owned};

pub struct MsQueue<T> {
    head: AtomicPtr<Node<T>>,
    tail: AtomicPtr<Node<T>>,
}

struct Node<T> {
    data: RefCell<Option<T>>,
    next: AtomicPtr<Node<T>>,
}

impl<T> MsQueue<T> {
    pub fn new() -> MsQueue<T> {
        let q = MsQueue { head: AtomicPtr::new(), tail: AtomicPtr::new() };
        let sentinel = Owned::new(Node {
            data: RefCell::new(None),
            next: AtomicPtr::new()
        });
        let guard = epoch::pin();
        let sentinel = q.head.store_and_ref(sentinel, Relaxed, &guard);
        unsafe { q.tail.store_shared(Some(sentinel), Relaxed); }
        q
    }

    pub fn push(&self, t: T) {
        let mut n = Owned::new(Node {
            data: RefCell::new(Some(t)),
            next: AtomicPtr::new()
        });
        let guard = epoch::pin();
        loop {
            let tail = self.tail.load(Acquire, &guard).unwrap();
            if let Some(next) = tail.next.load(Relaxed, &guard) {
                unsafe { self.tail.cas_shared(Some(tail), Some(next), Relaxed); }
                continue;
            }

            match tail.next.cas_and_ref(None, n, Release, &guard) {
                Ok(shared) => {
                    unsafe { self.tail.cas_shared(Some(tail), Some(shared), Relaxed); }
                    break;
                }
                Err(owned) => {
                    n = owned;
                }
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            let head = self.head.load(Acquire, &guard).unwrap();

            if let Some(next) = head.next.load(Relaxed, &guard) {
                if unsafe { self.head.cas_shared(Some(head), Some(next), Relaxed) } {
                    unsafe { guard.unlinked(head); }
                    // FIXME: rustc bug that (*next) is required
                    return (*next).data.borrow_mut().take()
                }
            } else {
                return None
            }
        }
    }
}

/*
impl<T: Debug> MsQueue<T> {
    pub fn debug(&self) {
        writeln!(stderr(), "Debugging queue:");

        let guard = epoch::pin();
        let mut node = self.head.load(Acquire, &guard);
        while let Some(n) = node {
            writeln!(stderr(), "{:?}", (*n).data.borrow());
            node = n.next.load(Relaxed, &guard);
        }

        writeln!(stderr(), "");
    }
}
*/

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use std::io::stderr;
    use std::io::prelude::*;

    use mem::epoch;
    use scope;
    use super::*;

    #[test]
    fn smoke_queue() {
        let q: MsQueue<i64> = MsQueue::new();
    }

    #[test]
    fn push_pop_1() {
        let q: MsQueue<i64> = MsQueue::new();
        q.push(37);
        assert_eq!(q.pop(), Some(37));
    }

    #[test]
    fn push_pop_2() {
        let q: MsQueue<i64> = MsQueue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.pop(), Some(37));
        assert_eq!(q.pop(), Some(48));
    }

    #[test]
    fn push_pop_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        for i in 0..200 {
            q.push(i)
        }
        for i in 0..200 {
            assert_eq!(q.pop(), Some(i));
        }
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: MsQueue<i64> = MsQueue::new();

        scope(|scope| {
            scope.spawn(|| {
                let mut next = 0;

                while next < CONC_COUNT {
                    if let Some(elem) = q.pop() {
                        assert_eq!(elem, next);
                        next += 1;
                    }
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        });
    }

    #[test]
    fn push_pop_many_spmc() {
        use std::time::Duration;

        fn recv(t: i32, q: &MsQueue<i64>) {
            let mut cur = -1;
            for i in 0..CONC_COUNT {
                if let Some(elem) = q.pop() {
                    if elem <= cur {
                        writeln!(stderr(), "{}: {} <= {}", t, elem, cur);
                    }
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 { break }
                }

                if i % 10000 == 0 {
                    //writeln!(stderr(), "{}: {} @ {}", t, i, cur);
                }
            }
        }

        let q: MsQueue<i64> = MsQueue::new();
        let qr = &q;
        scope(|scope| {
            for i in 0..3 {
                scope.spawn(move || recv(i, qr));
            }

            scope.spawn(|| {
                for i in 0..CONC_COUNT {
                    q.push(i);

                    if i % 10000 == 0 {
                        //writeln!(stderr(), "Push: {}", i);
                    }
                }
            })
        });
    }

    #[test]
    fn push_pop_many_mpmc() {
        enum LR { Left(i64), Right(i64) }

        let q: MsQueue<LR> = MsQueue::new();

        scope(|scope| {
            for _t in 0..2 {
                scope.spawn(|| {
                    for i in CONC_COUNT-1..CONC_COUNT {
                        q.push(LR::Left(i))
                    }
                });
                scope.spawn(|| {
                    for i in CONC_COUNT-1..CONC_COUNT {
                        q.push(LR::Right(i))
                    }
                });
                scope.spawn(|| {
                    let mut vl = vec![];
                    let mut vr = vec![];
                    for _i in 0..CONC_COUNT {
                        match q.pop() {
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
        });
    }
}
