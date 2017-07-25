use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

use {Atomic, Owned, Ptr, Scope};

/// An entry in the linked list.
pub struct Entry<T> {
    /// The data in the entry.
    data: T,

    /// The next entry in the linked list.
    /// If the tag is 1, this entry is marked as deleted.
    next: Atomic<Entry<T>>,
}

pub struct List<T> {
    head: Atomic<Entry<T>>,
}

pub struct Iter<'scope, T: 'scope> {
    /// The scope in which the iterator is operating.
    scope: &'scope Scope,

    /// Pointer to the head.
    head: &'scope Atomic<Entry<T>>,

    /// Pointer from the predecessor to the current entry.
    pred: &'scope Atomic<Entry<T>>,

    /// The current entry.
    curr: Ptr<'scope, Entry<T>>,
}

impl<T> Entry<T> {
    /// Returns the data in this entry.
    pub fn get(&self) -> &T {
        &self.data
    }

    /// Marks this entry as deleted.
    pub fn delete(&self, scope: &Scope) {
        self.next.fetch_or(1, Release, scope);
    }
}

impl<T> List<T> {
    /// Returns a new, empty linked list.
    pub fn new() -> List<T> {
        List { head: Atomic::null() }
    }

    /// Inserts `data` into the list.
    pub fn insert<'scope>(&self, data: T, scope: &'scope Scope) -> Ptr<'scope, Entry<T>> {
        let mut new = Owned::new(Entry {
            data: data,
            next: Atomic::null(),
        });
        let mut head = self.head.load(Relaxed, scope);

        loop {
            new.next.store(head, Relaxed);
            match self.head.compare_and_set_weak_owned(head, new, Release, scope) {
                Ok(n) => return n,
                Err((h, n)) => {
                    head = h;
                    new = n;
                }
            }
        }
    }

    /// Returns an iterator over all data.
    ///
    /// Every datum that is inserted at the moment this function is called and persists at least
    /// until the end of iteration will be returned. Since this iterator traverses a lock-free linked
    /// list that may be concurrently modified, some additional caveats apply:
    ///
    /// 1. If a new datum is inserted during iteration, it may or may not be returned.
    /// 2. If a datum is deleted during iteration, it may or may not be returned.
    /// 3. Any datum that gets returned may be returned multiple times.
    pub fn iter<'scope>(&'scope self, scope: &'scope Scope) -> Iter<'scope, T> {
        let head = &self.head;
        let pred = head;
        let curr = pred.load(Acquire, scope);
        Iter { scope, head, pred, curr }
    }
}

impl<'scope, T> Iterator for Iter<'scope, T> {
    type Item = &'scope T;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(c) = unsafe { self.curr.as_ref() } {
            let succ = c.next.load(Acquire, self.scope);

            if succ.tag() == 1 {
                // This entry was removed. Try unlinking it from the list.
                let succ = succ.with_tag(0);

                if self.pred
                    .compare_and_set(self.curr, succ, Release, self.scope)
                    .is_err()
                {
                    // We lost the race to unlink this entry - let's start over from the beginning.
                    self.pred = self.head;
                    self.curr = self.pred.load(Acquire, self.scope);
                    continue;
                }

                // FIXME(jeehoonkang): call `drop` for the unlinked entry.

                // Move forward, but don't change the predecessor.
                self.curr = succ;
            } else {
                // Move one step forward.
                self.pred = &c.next;
                self.curr = succ;

                return Some(&c.data);
            }
        }

        // We reached the end of the list.
        None
    }
}
