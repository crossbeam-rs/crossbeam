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
        let mut q = Queue { head: AtomicPtr::new(), tail: AtomicPtr::new() };
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
        let mut n = Some(Owned::new(Node {
            data: RefCell::new(Some(t)),
            next: AtomicPtr::new()
        }));
        let guard = epoch::pin();
        loop {
            let tail = self.tail.load(Acquire, &guard).unwrap();
            if let Some(next) = tail.next.load(Relaxed, &guard) {
                unsafe { self.tail.cas_shared(Some(tail), Some(next), Relaxed); }
            } else if let Err(owned) = tail.next.cas(None, n, Release) {
                n = owned;
            } else {
                break;
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn smoke_test() {
        let q: Queue<i32> = Queue::new();
        //q.push(3);
        //assert_eq!(q.pop(), Some(3));
    }
}
