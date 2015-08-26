use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::ptr;

use mem::epoch::{self, Atomic, Owned};

pub struct TreiberStack<T> {
    head: Atomic<Node<T>>,
}

struct Node<T> {
    data: T,
    next: Atomic<Node<T>>,
}

impl<T> TreiberStack<T> {
    pub fn new() -> TreiberStack<T> {
        TreiberStack {
            head: Atomic::new()
        }
    }

    pub fn push(&self, t: T) {
        let mut n = Owned::new(Node {
            data: t,
            next: Atomic::new()
        });
        let guard = epoch::pin();
        loop {
            let head = self.head.load(Relaxed, &guard);
            unsafe { n.next.store_shared(head, Relaxed); }
            match self.head.cas_and_ref(head, n, Release, &guard) {
                Ok(_) => break,
                Err(owned) => n = owned,
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            match self.head.load(Acquire, &guard) {
                Some(head) => {
                    let next = head.next.load(Relaxed, &guard);
                    let success = unsafe {
                        self.head.cas_shared(Some(head), next, Release)
                    };
                    if success {
                        return Some(unsafe { ptr::read(&(*head).data) })
                    }
                }
                None => return None
            }
        }
    }
}
