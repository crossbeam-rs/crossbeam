//! The global garbage collection realm
//!
//! This is the default realm for garbage collection.  For each thread, a mutator is lazily
//! initialized on its first use, thus registering the current thread.  If initialized, the
//! thread's mutator will get destructed on thread exit, which in turn unregisters the thread.
//!
//! `registries` is the list is the registered mutators, and `epoch` is the global epoch.

use mutator::{Mutator, LocalEpoch, Scope};
use epoch::Epoch;
use sync::list::List;


// FIXME(jeehoonkang): accessing globals in `lazy_static!` is blocking.
//
// Since static globals defined in `lazy_static!` are never dropped
// (https://github.com/rust-lang/rfcs/blob/master/text/1440-drop-types-in-const.md), it is safe to
// use `unprotected()` in `List`'s destructor.
lazy_static! {
    /// REGISTRIES is the head pointer of the list of mutator registries.
    pub static ref REGISTRIES: List<LocalEpoch> = List::new();
    /// EPOCH is a reference to the global epoch.
    pub static ref EPOCH: Epoch = Epoch::new();
}


/// Collect several bags from the global old garbage queue and destroys their objects.
/// Note: This may itself produce garbage and in turn allocate new bags.
pub fn collect(scope: &Scope) {
    unimplemented!()
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
