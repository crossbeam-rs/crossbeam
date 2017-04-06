//! Treiber stacks.

use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::ptr;

use epoch::{self, Atomic};

/// Treiber's lock-free stack.
///
/// This is usable with any number of producers and consumers.
#[derive(Debug)]
pub struct TreiberStack<T> {
    /// The head (top) node.
    head: Atomic<Node<T>>,
}

/// A node in the stack.
#[derive(Debug)]
struct Node<T> {
    /// The payload of this node.
    data: T,
    /// The node below this node.
    next: Atomic<Node<T>>,
}

impl<T> TreiberStack<T> {
    /// Create a new, empty stack.
    pub fn new() -> TreiberStack<T> {
        TreiberStack {
            head: Atomic::null(),
        }
    }

    /// Push an element on top of the stack.
    pub fn push(&self, elem: T) {
        // Construct the node.
        let mut n = Box::new(Node {
            data: elem,
            next: Atomic::null(),
        });

        // Pin the epoch.
        let guard = epoch::pin();
        // In order to solve the ABA condition, we spin until the CAS succeeds.
        loop {
            // Load the head node.
            let head = self.head.load(Relaxed, &guard);
            // Store the constructed head as the tail of the constructed node.
            n.next.store_shared(head, Relaxed);
            // CAS the head to the constructed head.
            match self.head.compare_and_set_ref(head, n, Release, &guard) {
                // Everything went well.
                Ok(_) => break,
                // It failed. Another thread changed the contents (pushed or popped) of the stack,
                // so we must repeat the proess.
                Err(owned) => n = owned,
            }
        }
    }

    /// Attempt to pop the top element of the stack.
    ///
    /// This returns `None` if the stack is observed to be empty.
    pub fn pop(&self) -> Option<T> {
        // Pin the epoch.
        let guard = epoch::pin();
        // Spin until the ABA condition is resolved.
        loop {
            // Load the node.
            match self.head.load(Acquire, &guard) {
                Some(head) => {
                    // Load the tail node.
                    let next = head.next.load(Relaxed, &guard);
                    // Set the node to the tail node, unless it changed (ABA condition).
                    if self.head.compare_and_set_shared(Some(head), next, Release) {
                        unsafe {
                            // Unlink the head node from the epoch.
                            guard.unlinked(head);
                            // Read the data.
                            return Some(ptr::read(&(*head).data));
                        }
                    }
                }
                // No nodes to pop.
                None => return None,
            }
        }
    }

    /// Check if this queue is empty.
    pub fn is_empty(&self) -> bool {
        // Pin the epoch.
        let guard = epoch::pin();
        // Test if the head is a null pointer.
        self.head.load(Acquire, &guard).is_none()
    }
}

impl<T> Default for TreiberStack<T> {
    fn default() -> TreiberStack<T> {
        TreiberStack::new()
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
        assert!(q.pop().is_some());
        assert!(q.pop().is_some());
        assert!(q.is_empty());
        q.push(25);
        assert!(!q.is_empty());
    }
}
