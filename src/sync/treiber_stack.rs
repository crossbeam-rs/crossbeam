use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::ptr;

use mem::epoch::{self, Atomic, Owned};

/// Treiber's lock-free stack.
///
/// Usable with any number of producers and consumers.
pub struct TreiberStack<T> {
    head: Atomic<Node<T>>,
}

struct Node<T> {
    data: T,
    next: Atomic<Node<T>>,
}

impl<T> TreiberStack<T> {
    /// Crate a new, empty stack.
    pub fn new() -> TreiberStack<T> {
        TreiberStack {
            head: Atomic::null()
        }
    }

    /// Push `t` on top of the stack.
    pub fn push(&self, t: T) {
        let mut n = Owned::new(Node {
            data: t,
            next: Atomic::null()
        });
        let guard = epoch::pin();
        loop {
            let head = self.head.load(Relaxed, &guard);
            n.next.store_shared(head, Relaxed);
            match self.head.cas_and_ref(head, n, Release, &guard) {
                Ok(_) => break,
                Err(owned) => n = owned,
            }
        }
    }

    /// Attempt to pop the top element of the stack.
    ///
    /// Returns `None` if the stack is observed to be empty.
    pub fn pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            match self.head.load(Acquire, &guard) {
                Some(head) => {
                    let next = head.next.load(Relaxed, &guard);
                    if self.head.cas_shared(Some(head), next, Release) {
                        unsafe {
                            guard.unlinked(head);
                            return Some(ptr::read(&(*head).data))
                        }
                    }
                }
                None => return None
            }
        }
    }
}
