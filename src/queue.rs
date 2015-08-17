use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::cell::RefCell;

use mem::epoch::{self, AtomicPtr, Owned};

pub struct Queue<T> {
    head: AtomicPtr<Node<T>>,
    tail: AtomicPtr<Node<T>>,
}

struct Node<T> {
    data: RefCell<Option<T>>,
    next: AtomicPtr<Node<T>>,
}

impl<T> Queue<T> {
    pub fn new() -> Queue<T> {
        let q = Queue { head: AtomicPtr::new(), tail: AtomicPtr::new() };
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
impl<T: Debug> Queue<T> {
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
    use std::thread;

    use super::*;

    #[test]
    fn smoke_queue() {
        let q: Queue<i32> = Queue::new();
    }

    #[test]
    fn push_pop_1() {
        let q: Queue<i32> = Queue::new();
        q.push(37);
        assert_eq!(q.pop(), Some(37));
    }

    #[test]
    fn push_pop_2() {
        let q: Queue<i32> = Queue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.pop(), Some(37));
        assert_eq!(q.pop(), Some(48));
    }

    #[test]
    fn push_pop_many_seq() {
        let q: Queue<i32> = Queue::new();
        for i in 0..200 {
            q.push(i)
        }
        for i in 0..200 {
            assert_eq!(q.pop(), Some(i));
        }
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: Queue<i32> = Queue::new();
        for i in 0..200 {
            q.push(i)
        }
        for i in 0..200 {
            assert_eq!(q.pop(), Some(i));
        }
    }
}
