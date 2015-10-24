use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::{ptr, mem};
use std::cmp;
use std::cell::UnsafeCell;

use mem::epoch::{self, Atomic, Owned};

const SEG_SIZE: usize = 32;

/// A Michael-Scott queue that allocates "segments" (arrays of nodes)
/// for efficiency.
///
/// Usable with any number of producers and consumers.
pub struct SegQueue<T> {
    head: Atomic<Segment<T>>,
    tail: Atomic<Segment<T>>,
}

struct Segment<T> {
    low: AtomicUsize,
    data: [UnsafeCell<T>; SEG_SIZE],
    ready: [AtomicBool; SEG_SIZE],
    high: AtomicUsize,
    next: Atomic<Segment<T>>,
}

unsafe impl<T> Sync for Segment<T> {}

impl<T> Segment<T> {
    fn new() -> Segment<T> {
        Segment {
            data: unsafe { mem::uninitialized() },
            ready: unsafe { mem::transmute([0usize; SEG_SIZE]) },
            low: AtomicUsize::new(0),
            high: AtomicUsize::new(0),
            next: Atomic::null(),
        }
    }
}

impl<T> SegQueue<T> {
    /// Create a new, empty queue.
    pub fn new() -> SegQueue<T> {
        let q = SegQueue {
            head: Atomic::null(),
            tail: Atomic::null(),
        };
        let sentinel = Owned::new(Segment::new());
        let guard = epoch::pin();
        let sentinel = q.head.store_and_ref(sentinel, Relaxed, &guard);
        q.tail.store_shared(Some(sentinel), Relaxed);
        q
    }

    /// Add `t` to the back of the queue.
    pub fn push(&self, t: T) {
        let guard = epoch::pin();
        loop {
            let tail = self.tail.load(Acquire, &guard).unwrap();
            if tail.high.load(Relaxed) >= SEG_SIZE { continue }
            let i = tail.high.fetch_add(1, Relaxed);
            unsafe {
                if i < SEG_SIZE {
                    *(*tail).data.get_unchecked(i).get() = t;
                    tail.ready.get_unchecked(i).store(true, Release);

                    if i + 1 == SEG_SIZE {
                        let tail = tail.next.store_and_ref(Owned::new(Segment::new()), Release, &guard);
                        self.tail.store_shared(Some(tail), Release);
                    }

                    return
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
            loop {
                let low = head.low.load(Relaxed);
                if low >= cmp::min(head.high.load(Relaxed), SEG_SIZE) { break }
                if head.low.compare_and_swap(low, low+1, Relaxed) == low {
                    loop {
                        if unsafe { head.ready.get_unchecked(low).load(Acquire) } { break }
                    }
                    if low + 1 == SEG_SIZE {
                        loop {
                            if let Some(next) = head.next.load(Acquire, &guard) {
                                self.head.store_shared(Some(next), Release);
                                break
                            }
                        }
                    }
                    return Some(unsafe { ptr::read((*head).data.get_unchecked(low).get()) })
                }
            }
            if head.next.load(Relaxed, &guard).is_none() { return None }
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
        let q: SegQueue<i64> = SegQueue::new();
    }

    #[test]
    fn push_pop_1() {
        let q: SegQueue<i64> = SegQueue::new();
        q.push(37);
        assert_eq!(q.pop(), Some(37));
    }

    #[test]
    fn push_pop_2() {
        let q: SegQueue<i64> = SegQueue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.pop(), Some(37));
        assert_eq!(q.pop(), Some(48));
    }

    #[test]
    fn push_pop_many_seq() {
        let q: SegQueue<i64> = SegQueue::new();
        for i in 0..200 {
            q.push(i)
        }
        writeln!(stderr(), "done pushing");
        for i in 0..200 {
            assert_eq!(q.pop(), Some(i));
        }
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: SegQueue<i64> = SegQueue::new();

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

        fn recv(t: i32, q: &SegQueue<i64>) {
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

        let q: SegQueue<i64> = SegQueue::new();
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

        let q: SegQueue<LR> = SegQueue::new();

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
