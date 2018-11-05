use std::cell::UnsafeCell;
use std::cmp;
use std::fmt;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicUsize};

use epoch::{self, Atomic, Owned};

const SEG_SIZE: usize = 32;

/// A Michael-Scott queue that allocates "segments" (arrays of nodes)
/// for efficiency.
///
/// Usable with any number of producers and consumers.
#[derive(Debug)]
pub struct SegQueue<T> {
    head: Atomic<Segment<T>>,
    tail: Atomic<Segment<T>>,
}

struct Segment<T> {
    low: AtomicUsize,
    data: ManuallyDrop<[UnsafeCell<(T, AtomicBool)>; SEG_SIZE]>,
    high: AtomicUsize,
    next: Atomic<Segment<T>>,
}

impl<T> fmt::Debug for Segment<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Segment {{ ... }}")
    }
}

unsafe impl<T: Send> Sync for Segment<T> {}

impl<T> Segment<T> {
    fn new() -> Segment<T> {
        let rqueue = Segment {
            data: unsafe { ManuallyDrop::new(mem::uninitialized()) },
            low: AtomicUsize::new(0),
            high: AtomicUsize::new(0),
            next: Atomic::null(),
        };
        for val in rqueue.data.iter() {
            unsafe {
                (*val.get()).1 = AtomicBool::new(false);
            }
        }
        rqueue
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
        let sentinel = sentinel.into_shared(&guard);
        q.head.store(sentinel, Relaxed);
        q.tail.store(sentinel, Relaxed);
        q
    }

    /// Add `t` to the back of the queue.
    pub fn push(&self, t: T) {
        let guard = epoch::pin();
        loop {
            let tail = unsafe { self.tail.load(Acquire, &guard).as_ref() }.unwrap();
            if tail.high.load(Relaxed) >= SEG_SIZE {
                continue;
            }
            let i = tail.high.fetch_add(1, Relaxed);
            unsafe {
                if i < SEG_SIZE {
                    let cell = (*tail).data.get_unchecked(i).get();
                    ptr::write(&mut (*cell).0, t);
                    (*cell).1.store(true, Release);

                    if i + 1 == SEG_SIZE {
                        let tail_new = Owned::new(Segment::new()).into_shared(&guard);
                        tail.next.store(tail_new, Release);
                        self.tail.store(tail_new, Release);
                    }

                    return;
                }
            }
        }
    }

    /// Judge if the queue is empty.
    ///
    /// Returns `true` if the queue is empty.
    pub fn is_empty(&self) -> bool {
        let guard = epoch::pin();
        let head = self.head.load(Acquire, &guard);
        let tail = self.tail.load(Acquire, &guard);
        if head != tail {
            return false;
        }

        let head_ref = unsafe { head.as_ref() }.unwrap();
        let low = head_ref.low.load(Relaxed);
        low >= cmp::min(head_ref.high.load(Relaxed), SEG_SIZE)
    }

    /// Attempt to dequeue from the front.
    ///
    /// Returns `None` if the queue is observed to be empty.
    pub fn try_pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            let head_shared = self.head.load(Acquire, &guard);
            let head = unsafe { head_shared.as_ref() }.unwrap();
            loop {
                let low = head.low.load(Relaxed);
                if low >= cmp::min(head.high.load(Relaxed), SEG_SIZE) {
                    break;
                }
                if head.low.compare_and_swap(low, low + 1, Relaxed) == low {
                    unsafe {
                        let cell = (*head).data.get_unchecked(low).get();
                        loop {
                            if (*cell).1.load(Acquire) {
                                break;
                            }
                        }
                        if low + 1 == SEG_SIZE {
                            loop {
                                let next_shared = head.next.load(Acquire, &guard);
                                if next_shared.as_ref().is_some() {
                                    self.head.store(next_shared, Release);
                                    guard.defer_destroy(head_shared);
                                    break;
                                }
                            }
                        }
                        return Some(ptr::read(&(*cell).0));
                    }
                }
            }
            if head.next.load(Relaxed, &guard).is_null() {
                return None;
            }
        }
    }
}

impl<T> Drop for SegQueue<T> {
    fn drop(&mut self) {
        while self.try_pop().is_some() {}

        // Destroy the remaining sentinel segment.
        let guard = epoch::pin();
        let sentinel = self.head.load(Relaxed, &guard).as_raw() as *mut Segment<T>;
        unsafe {
            drop(Vec::from_raw_parts(sentinel, 0, 1));
        }
    }
}

impl<T> Default for SegQueue<T> {
    fn default() -> Self {
        SegQueue::new()
    }
}

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use super::*;
    use scope;

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
                scope.spawn(|_| recv(i, &q));
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
}
