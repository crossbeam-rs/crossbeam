//! Garbage collection.
//!
//! # Garbages
//!
//! Objects that get unlinked from concurrent data structures must be stashed away until the global
//! epoch sufficiently advances so that they become safe for destruction.  We call these objects
//! garbages.  When the global epoch advances sufficiently, `Destroy` garbages are dropped (i.e. the
//! destructors are called), and `Free` garbages are freed.  In addition, you can register arbitrary
//! function to be called later using the `Fn` garbages.
//!
//! # Bags
//!
//! Pointers to such garbages are pushed into thread-local bags, and when it becomes full, the bag
//! is marked with the current global epoch and pushed into a global queue of garbage bags.  We
//! store garbages in thread-local storages for amortizing the synchronization cost of pushing the
//! garbages to a global queue.
//!
//! # Garbage queues
//!
//! Whenever a bag is pushed into a queue, some garbage in the queue is collected and destroyed
//! along the way.  This design reduces contention on data structures.  The global queue cannot be
//! explicitly accessed: the only way to interact with it is by calling functions `defer*()`, or
//! calling `collect()` that manually triggers garbage collection.  Ideally each instance of
//! concurrent data structure may have its own queue that gets fully destroyed as soon as the data
//! structure gets dropped.

use core::fmt;
use arrayvec::ArrayVec;
use deferred::Deferred;

/// Maximum number of objects a bag can contain.
#[cfg(not(feature = "strict_gc"))]
const MAX_OBJECTS: usize = 64;
#[cfg(feature = "strict_gc")]
const MAX_OBJECTS: usize = 4;


pub struct Garbage {
    func: Deferred,
}


unsafe impl Sync for Garbage {}
unsafe impl Send for Garbage {}

impl fmt::Debug for Garbage {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "garbage {{ ... }}")
    }
}

impl Garbage {
    /// Make a closure that will later be called.
    pub fn new<F: FnOnce()>(f: F) -> Self {
        Garbage { func: Deferred::new(move || f()) }
    }
}

impl Drop for Garbage {
    fn drop(&mut self) {
        self.func.call();
    }
}


/// Bag of garbages.
#[derive(Default, Debug)]
pub struct Bag {
    /// Stashed objects.
    objects: ArrayVec<[Garbage; MAX_OBJECTS]>,
}

impl Bag {
    /// Returns a new, empty bag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the bag is empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Attempts to insert a garbage object into the bag and returns `true` if succeeded.
    pub fn try_push(&mut self, garbage: Garbage) -> Result<(), Garbage> {
        self.objects.try_push(garbage).map_err(|e| e.element())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT};
    use std::sync::atomic::Ordering;

    use super::{Garbage, Bag};

    #[test]
    fn check_defer() {
        static FLAG: AtomicUsize = ATOMIC_USIZE_INIT;
        fn set() {
            FLAG.store(42, Ordering::Relaxed);
        }

        let g = Garbage::new(set);
        assert_eq!(FLAG.load(Ordering::Relaxed), 0);
        drop(g);
        assert_eq!(FLAG.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn check_bag() {
        static FLAG: AtomicUsize = ATOMIC_USIZE_INIT;
        fn incr() {
            FLAG.fetch_add(1, Ordering::Relaxed);
        }

        let mut bag = Bag::new();
        assert!(bag.is_empty());

        for _ in 0..super::MAX_OBJECTS {
            assert!(bag.try_push(Garbage::new(incr)).is_ok());
            assert!(!bag.is_empty());
            assert_eq!(FLAG.load(Ordering::Relaxed), 0);
        }

        let result = bag.try_push(Garbage::new(incr));
        assert!(result.is_err());
        assert!(!bag.is_empty());
        assert_eq!(FLAG.load(Ordering::Relaxed), 0);

        drop(bag);
        assert_eq!(FLAG.load(Ordering::Relaxed), super::MAX_OBJECTS);
    }
}
