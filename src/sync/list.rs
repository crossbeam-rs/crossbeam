//! Michael's lock-free linked list.
//!
//! Michael.  High Performance Dynamic Lock-Free Hash Tables and List-Based Sets.  SPAA 2002.
//! http://dl.acm.org/citation.cfm?id=564870.564881

use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

use {Atomic, Owned, Ptr, Scope, unprotected};
use crossbeam_utils::cache_padded::CachePadded;


/// An entry in the linked list.
struct NodeInner<T> {
    /// The data in the entry.
    data: T,

    /// The next entry in the linked list.
    /// If the tag is 1, this entry is marked as deleted.
    next: Atomic<Node<T>>,
}

pub struct Node<T>(CachePadded<NodeInner<T>>);

pub struct List<T> {
    head: Atomic<Node<T>>,
}

pub struct Iter<'scope, T: 'scope> {
    /// The scope in which the iterator is operating.
    scope: &'scope Scope,

    /// Pointer from the predecessor to the current entry.
    pred: &'scope Atomic<Node<T>>,

    /// The current entry.
    curr: Ptr<'scope, Node<T>>,
}

pub enum IterResult<'scope, T: 'scope> {
    Some(&'scope T),
    None,
    Abort,
}

impl<T> Node<T> {
    /// Returns the data in this entry.
    fn new(data: T) -> Self {
        Node(CachePadded::new(NodeInner {
            data: data,
            next: Atomic::null(),
        }))
    }

    pub fn get(&self) -> &T {
        &self.0.data
    }

    /// Marks this entry as deleted.
    pub fn delete<'scope>(&self, scope: &Scope) {
        self.0.next.fetch_or(1, Release, scope);
    }
}

impl<T> List<T> {
    /// Returns a new, empty linked list.
    pub fn new() -> Self {
        List { head: Atomic::null() }
    }

    /// Inserts `data` into the list.
    pub fn insert<'scope>(
        &'scope self,
        to: &'scope Atomic<Node<T>>,
        data: T,
        scope: &'scope Scope,
    ) -> Ptr<'scope, Node<T>> {
        let mut cur = Owned::new(Node::new(data));
        let mut next = to.load(Relaxed, scope);

        loop {
            cur.0.next.store(next, Relaxed);
            match to.compare_and_set_weak_owned(next, cur, Release, scope) {
                Ok(cur) => return cur,
                Err((n, c)) => {
                    next = n;
                    cur = c;
                }
            }
        }
    }

    pub fn insert_head<'scope>(
        &'scope self,
        data: T,
        scope: &'scope Scope,
    ) -> Ptr<'scope, Node<T>> {
        self.insert(&self.head, data, scope)
    }

    /// Returns an iterator over all data.
    ///
    /// Every datum that is inserted at the moment this function is called and persists at least
    /// until the end of iteration will be returned. Since this iterator traverses a lock-free
    /// linked list that may be concurrently modified, some additional caveats apply:
    ///
    /// 1. If a new datum is inserted during iteration, it may or may not be returned.
    /// 2. If a datum is deleted during iteration, it may or may not be returned.
    /// 3. It may not return all data if a concurrent thread continues to iterate the same list.
    pub fn iter<'scope>(&'scope self, scope: &'scope Scope) -> Iter<'scope, T> {
        let pred = &self.head;
        let curr = pred.load(Acquire, scope);
        Iter { scope, pred, curr }
    }
}

impl<T> Drop for List<T> {
    fn drop(&mut self) {
        unsafe {
            unprotected(|scope| {
                let mut curr = self.head.load(Relaxed, scope);
                while let Some(c) = curr.as_ref() {
                    let succ = c.0.next.load(Relaxed, scope);
                    drop(Box::from_raw(curr.as_raw() as *mut Node<T>));
                    curr = succ;
                }
            });
        }
    }
}

impl<'scope, T> Iter<'scope, T> {
    pub fn next(&mut self) -> IterResult<T> {
        while let Some(c) = unsafe { self.curr.as_ref() } {
            let succ = c.0.next.load(Acquire, self.scope);

            if succ.tag() == 1 {
                // This entry was removed. Try unlinking it from the list.
                let succ = succ.with_tag(0);

                match self.pred.compare_and_set_weak(
                    self.curr,
                    succ,
                    Acquire,
                    self.scope,
                ) {
                    Ok(_) => {
                        unsafe {
                            self.scope.defer_free(self.curr);
                        }
                        self.curr = succ;
                    }
                    Err(_) => {
                        // We lost the race to delete the entry.  Since another thread trying
                        // to iterate the list has won the race, we return early.
                        return IterResult::Abort;
                    }
                }

                continue;
            }

            // Move one step forward.
            self.pred = &c.0.next;
            self.curr = succ;

            return IterResult::Some(&c.0.data);
        }

        // We reached the end of the list.
        IterResult::None
    }
}

impl<T> Default for List<T> {
    fn default() -> Self {
        Self::new()
    }
}
