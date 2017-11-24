//! Lock-free intrusive linked list.
//!
//! Ideas from Michael.  High Performance Dynamic Lock-Free Hash Tables and List-Based Sets.  SPAA
//! 2002.  http://dl.acm.org/citation.cfm?id=564870.564881

use std::marker::PhantomData;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

use {Atomic, Shared, Guard, unprotected};

/// An entry in a linked list.
///
/// An Entry is accessed from multiple threads, so it would be beneficial to put it in a different
/// cache-line than thread-local data in terms of performance.
#[derive(Debug)]
pub struct Entry {
    /// The next entry in the linked list.
    /// If the tag is 1, this entry is marked as deleted.
    next: Atomic<Entry>,
}

/// An evidence that the type `T` contains an entry.
///
/// Suppose we'll maintain an intrusive linked list of type `T`. Since `List` and `Entry` types do
/// not provide any information on `T`, we need to specify how to interact with the containing
/// objects of type `T`. A struct of this trait provides such information.
///
/// `container_of()` is given a pointer to an entry, and returns the pointer to its container. On
/// the other hand, `entry_of()` is given a pointer to a container, and returns the pointer to its
/// corresponding entry. `finalize()` is called when an element is actually removed from the list.
///
/// # Example
///
/// ```ignore
/// struct A {
///     entry: Entry,
///     data: usize,
/// }
///
/// struct AEntry {}
///
/// impl Container<A> for AEntry {
///     fn container_of(entry: *const Entry) -> *const A {
///         ((entry as usize) - offset_of!(A, entry)) as *const _
///     }
///
///     fn entry_of(a: *const A) -> *const Entry {
///         ((a as usize) + offset_of!(A, entry)) as *const _
///     }
///
///     fn finalize(entry: *const Entry) {
///         // drop the box of the container
///         unsafe { drop(Box::from_raw(Self::container_of(entry) as *mut A)) }
///     }
/// }
/// ```
///
/// Note that there can be multiple structs that implement `Container<T>`. In most cases, each
/// struct will represent a distinct entry in `T` so that the container can be inserted into
/// multiple lists. For example, we can insert the following struct into two lists using `entry1`
/// and `entry2` ans its entry:
///
/// ```ignore
/// struct B {
///     entry1: Entry,
///     entry2: Entry,
///     data: usize,
/// }
/// ```
pub trait Container<T> {
    fn container_of(*const Entry) -> *const T;
    fn entry_of(*const T) -> *const Entry;
    fn finalize(*const Entry);
}

/// A lock-free, intrusive linked list of type `T`.
#[derive(Debug)]
pub struct List<T, C: Container<T>> {
    /// The head of the linked list.
    head: Atomic<Entry>,

    /// The phantom data for using `T` and `E`.
    _marker: PhantomData<(T, C)>,
}

/// An auxiliary data for iterating over a linked list.
pub struct Iter<'g, T: 'g, C: Container<T>> {
    /// The guard that protects the iteration.
    guard: &'g Guard,

    /// Pointer from the predecessor to the current entry.
    pred: &'g Atomic<Entry>,

    /// The current entry.
    curr: Shared<'g, Entry>,

    /// The phantom data for container.
    _marker: PhantomData<(&'g T, C)>,
}

/// An enum for iteration error.
#[derive(PartialEq, Debug)]
pub enum IterError {
    /// Iterator was stalled by another iterator. Internally, the thread lost a race in deleting a
    /// node by a concurrent thread.
    Stalled,
}

impl Default for Entry {
    /// Returns the empty entry.
    fn default() -> Self {
        Self { next: Atomic::null() }
    }
}

impl Entry {
    /// Marks this entry as deleted, deferring the actual deallocation to a later iteration.
    ///
    /// # Safety
    ///
    /// The entry should be a member of a linked list, and it should not have been deleted. It
    /// should be safe to call `C::finalize` on the entry after the `guard` is dropped, where `C` is
    /// the associated helper for the linked list.
    pub unsafe fn delete(&self, guard: &Guard) {
        self.next.fetch_or(1, Release, guard);
    }
}

impl<T, C: Container<T>> List<T, C> {
    /// Returns a new, empty linked list.
    pub fn new() -> Self {
        Self {
            head: Atomic::null(),
            _marker: PhantomData,
        }
    }

    /// Inserts `entry` into the head of the list.
    ///
    /// # Safety
    ///
    /// You should guarantee that:
    ///
    /// - `C::entry_of()` properly accesses an `Entry` in the container;
    /// - `C::container_of()` properly accesses the object containing the entry;
    /// - `container` is immovable, e.g. inside a `Box`;
    /// - An entry is not inserted twice; and
    /// - The inserted object will be removed before the list is dropped.
    pub unsafe fn insert<'g>(&'g self, container: Shared<'g, T>, guard: &'g Guard) {
        let to = &self.head;
        let entry = &*C::entry_of(container.as_raw());
        let entry_ptr = Shared::from(entry as *const _);
        let mut next = to.load(Relaxed, guard);

        loop {
            entry.next.store(next, Relaxed);
            match to.compare_and_set_weak(next, entry_ptr, Release, guard) {
                Ok(_) => break,
                Err(err) => next = err.new,
            }
        }
    }

    /// Returns an iterator over all objects.
    ///
    /// # Caveat
    ///
    /// Every object that is inserted at the moment this function is called and persists at least
    /// until the end of iteration will be returned. Since this iterator traverses a lock-free
    /// linked list that may be concurrently modified, some additional caveats apply:
    ///
    /// 1. If a new object is inserted during iteration, it may or may not be returned.
    /// 2. If an object is deleted during iteration, it may or may not be returned.
    /// 3. The iteration may be aborted when it lost in a race condition. In this case, the winning
    ///    thread will continue to iterate over the same list.
    pub fn iter<'g>(&'g self, guard: &'g Guard) -> Iter<'g, T, C> {
        let pred = &self.head;
        let curr = pred.load(Acquire, guard);
        Iter {
            guard,
            pred,
            curr,
            _marker: PhantomData,
        }
    }
}

impl<T, C: Container<T>> Drop for List<T, C> {
    fn drop(&mut self) {
        unsafe {
            let guard = &unprotected();
            let mut curr = self.head.load(Relaxed, guard);
            while let Some(c) = curr.as_ref() {
                let succ = c.next.load(Relaxed, guard);
                assert_eq!(succ.tag(), 1);

                C::finalize(curr.as_raw());
                curr = succ;
            }
        }
    }
}

impl<'g, T: 'g, C: Container<T>> Iterator for Iter<'g, T, C> {
    type Item = Result<&'g T, IterError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(c) = unsafe { self.curr.as_ref() } {
            let succ = c.next.load(Acquire, self.guard);

            if succ.tag() == 1 {
                // This entry was removed. Try unlinking it from the list.
                let succ = succ.with_tag(0);

                match self.pred.compare_and_set_weak(
                    self.curr,
                    succ,
                    Acquire,
                    self.guard,
                ) {
                    Ok(_) => {
                        unsafe {
                            // Deferred drop of `T` is scheduled here.
                            // This is okay because `.delete()` can be called only if `T: 'static`.
                            let p = self.curr;
                            self.guard.defer(move || C::finalize(p.as_raw()));
                        }
                        self.curr = succ;
                    }
                    Err(err) => {
                        // We lost the race to delete the entry by a concurrent iterator. Set
                        // `self.curr` to the updated pointer, and report that we are stalled.
                        self.curr = err.new;
                        return Some(Err(IterError::Stalled));
                    }
                }

                continue;
            }

            // Move one step forward.
            self.pred = &c.next;
            self.curr = succ;

            return Some(Ok(unsafe { &*C::container_of(c as *const _) }));
        }

        // We reached the end of the list.
        None
    }
}

#[cfg(test)]
mod tests {
    use {Collector, Owned};
    use super::*;

    #[test]
    fn insert_iter_delete_iter() {
        let collector = Collector::new();
        let handle = collector.handle();
        let guard = handle.pin();

        struct EntryContainer {}

        impl Container<Entry> for EntryContainer {
            fn container_of(entry: *const Entry) -> *const Entry {
                entry
            }

            fn entry_of(entry: *const Entry) -> *const Entry {
                entry
            }

            fn finalize(entry: *const Entry) {
                unsafe { drop(Box::from_raw(entry as *mut Entry)) }
            }
        }

        let l: List<Entry, EntryContainer> = List::new();

        let n1 = Owned::new(Entry::default()).into_shared(&guard);
        let n2 = Owned::new(Entry::default()).into_shared(&guard);
        let n3 = Owned::new(Entry::default()).into_shared(&guard);

        unsafe {
            l.insert(n3, &guard);
            l.insert(n2, &guard);
            l.insert(n1, &guard);
        }

        let mut iter = l.iter(&guard);
        assert!(iter.next().is_some());
        assert!(iter.next().is_some());
        assert!(iter.next().is_some());
        assert!(iter.next().is_none());

        unsafe {
            n2.as_ref().unwrap().delete(&guard);
        }

        let mut iter = l.iter(&guard);
        assert!(iter.next().is_some());
        assert!(iter.next().is_some());
        assert!(iter.next().is_none());

        unsafe {
            n1.as_ref().unwrap().delete(&guard);
            n3.as_ref().unwrap().delete(&guard);
        }

        let mut iter = l.iter(&guard);
        assert!(iter.next().is_none());
    }
}
