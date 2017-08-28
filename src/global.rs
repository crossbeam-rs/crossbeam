//! The global garbage collection realm
//!
//! This is the default realm for garbage collection.  For each thread, a mutator is lazily
//! initialized on its first use, thus registering the current thread.  If initialized, the
//! thread's mutator will get destructed on thread exit, which in turn unregisters the thread.
//!
//! `registries` is the list is the registered mutators, and `epoch` is the global epoch.

use std::cmp;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use mutator::{Mutator, LocalEpoch, Scope, unprotected_with_bag};
use garbage::Bag;
use epoch::Epoch;
use sync::list::List;
use sync::queue::Queue;


/// Number of bags to destroy.
const COLLECT_STEPS: usize = 8;


// FIXME(jeehoonkang): accessing globals in `lazy_static!` is blocking.
//
// Since static globals defined in `lazy_static!` are never dropped
// (https://github.com/rust-lang/rfcs/blob/master/text/1440-drop-types-in-const.md), it is safe to
// use `unprotected()` in `List`'s destructor.
lazy_static! {
    /// REGISTRIES is the head pointer of the list of mutator registries.
    pub static ref REGISTRIES: List<LocalEpoch> = List::new();
    /// GARBAGES is a reference to the global queue of garbages.
    pub static ref GARBAGES: Queue<(usize, Bag)> = Queue::new();
    /// EPOCH is a reference to the global epoch.
    pub static ref EPOCH: Epoch = Epoch::new();
}


/// Pushes the bag onto the global queue and replaces the bag with a new empty bag.
#[inline]
pub fn push_bag<'scope>(bag: &mut Bag, scope: &'scope Scope) {
    let epoch = EPOCH.load(Relaxed);
    let bag = ::std::mem::replace(bag, Bag::new());
    ::std::sync::atomic::fence(SeqCst);
    GARBAGES.push((epoch, bag), scope);
}

/// Collect several bags from the global old garbage queue and destroys their objects.
///
/// Note: This may itself produce garbage and in turn allocate new bags.
pub fn collect(scope: &Scope) {
    let epoch = EPOCH.try_advance(&REGISTRIES, scope);

    let condition = |bag: &(usize, Bag)| {
        // A pinned thread can witness at most one epoch advancement. Therefore, any bag that is
        // within one epoch of the current one cannot be destroyed yet.
        let diff = epoch.wrapping_sub(bag.0);
        cmp::min(diff, 0usize.wrapping_sub(diff)) > 2
    };

    let garbages = &GARBAGES;
    for _ in 0..COLLECT_STEPS {
        match garbages.try_pop_if(&condition, scope) {
            None => break,
            Some(bag) => drop(bag),
        }
    }
}


thread_local! {
    /// The per-thread mutator.
    static MUTATOR: Mutator<'static> = Mutator::new();
}

/// Pin the current thread.
pub fn pin<F, R>(f: F) -> R
where
    F: FnOnce(&Scope) -> R,
{
    MUTATOR.with(|mutator| mutator.pin(f))
}

/// Check if the current thread is pinned.
pub fn is_pinned() -> bool {
    MUTATOR.with(|mutator| mutator.is_pinned())
}

/// Returns a [`Scope`] without pinning any mutator.
///
/// Sometimes, we'd like to have longer-lived scopes in which we know our thread is the only one
/// accessing atomics. This is true e.g. when destructing a big data structure, or when constructing
/// it from a long iterator. In such cases we don't need to be overprotective because there is no
/// fear of other threads concurrently destroying objects.
///
/// Function `unprotected` is *unsafe* because we must promise that no other thread is accessing the
/// Atomics and objects at the same time. The function is safe to use only if (1) the locations that
/// we access should not be deallocated by concurrent mutators, and (2) the locations that we
/// deallocate should not be accessed by concurrent mutators.
///
/// Just like with the safe epoch::pin function, unprotected use of atomics is enclosed within a
/// scope so that pointers created within it don't leak out or get mixed with pointers from other
/// scopes.
pub unsafe fn unprotected<F, R>(f: F) -> R
where
    F: FnOnce(&Scope) -> R,
{
    let mut bag = Bag::new();
    let result = unprotected_with_bag(&mut bag, f);
    result
}


#[cfg(test)]
mod tests {
    use std::thread;
    use std::sync::atomic::Ordering::Relaxed;

    use super::*;

    #[test]
    fn pin_reentrant() {
        assert!(!is_pinned());
        pin(|_| {
            pin(|_| {
                assert!(is_pinned());
            });
            assert!(is_pinned());
        });
        assert!(!is_pinned());
    }

    #[test]
    fn pin_holds_advance() {
        let threads = (0..8)
            .map(|_| {
                thread::spawn(|| for _ in 0..500_000 {
                    pin(|scope| {
                        let before = EPOCH.load(Relaxed);
                        EPOCH.try_advance(&REGISTRIES, scope);
                        let after = EPOCH.load(Relaxed);

                        assert!(after.wrapping_sub(before) <= 2);
                    });
                })
            })
            .collect::<Vec<_>>();

        for t in threads {
            t.join().unwrap();
        }
    }
}
