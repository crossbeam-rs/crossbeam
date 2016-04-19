use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::ptr;

use mem::epoch::{self, Atomic, Owned};

/// Treiber's lock-free stack.
///
/// Usable with any number of producers and consumers.
#[derive(Debug)]
pub struct TreiberStack<T> {
    head: Atomic<Node<T>>,
}

#[derive(Debug)]
struct Node<T> {
    data: T,
    next: Atomic<Node<T>>,
}

impl<T> TreiberStack<T> {
    /// Create a new, empty stack.
    pub fn new() -> TreiberStack<T> {
        TreiberStack { head: Atomic::null() }
    }

    /// Push `t` on top of the stack.
    pub fn push(&self, t: T) {
        let mut n = Owned::new(Node {
            data: t,
            next: Atomic::null(),
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
    /// **Deprecated method**, use try_pop
    ///
    /// Returns `None` if the stack is observed to be empty.
    #[cfg_attr(any(feature="beta", feature="nightly"), deprecated(note="The pop method has been renamed to try_pop for consistency with other collections."))]
    pub fn pop(&self) -> Option<T> {
        self.try_pop()
    }

    /// Attempt to pop the top element of the stack.
    ///
    /// Returns `None` if the stack is observed to be empty.
    pub fn try_pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            match self.head.load(Acquire, &guard) {
                Some(head) => {
                    let next = head.next.load(Relaxed, &guard);
                    if self.head.cas_shared(Some(head), next, Release) {
                        unsafe {
                            guard.unlinked(head);
                            return Some(ptr::read(&(*head).data));
                        }
                    }
                }
                None => return None,
            }
        }
    }

    /// Check if this queue is empty.
    pub fn is_empty(&self) -> bool {
        let guard = epoch::pin();
        self.head.load(Acquire, &guard).is_none()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn is_empty() {
        let q: TreiberStack<i64> = TreiberStack::new();
        assert!(q.is_empty());
        q.push(20);
        q.push(20);
        assert!(!q.is_empty());
        assert!(!q.is_empty());
        assert!(q.try_pop().is_some());
        assert!(q.try_pop().is_some());
        assert!(q.is_empty());
        q.push(25);
        assert!(!q.is_empty());
    }
}
