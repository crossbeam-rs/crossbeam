use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::{ptr, mem};

use mem::epoch::{self, Atomic, Owned};
use mem::CachePadded;

/// A Michael-Scott lock-free queue.
///
/// Usable with any number of producers and consumers.
pub struct MsQueue<T> {
    head: CachePadded<Atomic<Node<T>>>,
    tail: CachePadded<Atomic<Node<T>>>,
}

struct Node<T> {
    data: T,
    next: Atomic<Node<T>>,
}

impl<T> MsQueue<T> {
    /// Create a new, empty queue.
    pub fn new() -> MsQueue<T> {
        let q = MsQueue {
            head: CachePadded::new(Atomic::null()),
            tail: CachePadded::new(Atomic::null()),
        };
        let sentinel = Owned::new(Node {
            data: unsafe { mem::uninitialized() },
            next: Atomic::null()
        });
        let guard = epoch::pin();
        let sentinel = q.head.store_and_ref(sentinel, Relaxed, &guard);
        q.tail.store_shared(Some(sentinel), Relaxed);
        q
    }

    /// Add `t` to the back of the queue.
    pub fn push(&self, t: T) {
        let mut n = Owned::new(Node {
            data: t,
            next: Atomic::null()
        });
        let guard = epoch::pin();
        loop {
            let tail = self.tail.load(Acquire, &guard).unwrap();
            if let Some(next) = tail.next.load(Acquire, &guard) {
                self.tail.cas_shared(Some(tail), Some(next), Release);
                continue;
            }

            match tail.next.cas_and_ref(None, n, Release, &guard) {
                Ok(shared) => {
                    self.tail.cas_shared(Some(tail), Some(shared), Release);
                    break;
                }
                Err(owned) => {
                    n = owned;
                }
            }
        }
    }

    /// Attempt to dequeue from the front.
    ///
    /// Returns `None` if the queue is observed to be empty.
    pub fn pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            let head = self.head.load(Acquire, &guard).unwrap();

            if let Some(next) = head.next.load(Acquire, &guard) {
                unsafe {
                    if self.head.cas_shared(Some(head), Some(next), Release) {
                        guard.unlinked(head);
                        return Some(ptr::read(&(*next).data))
                    }
                }
            } else {
                return None
            }
        }
    }
}

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
