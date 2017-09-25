//! The global epoch
//!
//! The last bit in this number is unused and is always zero. Every so often the global epoch is
//! incremented, i.e. we say it "advances". A pinned mutator may advance the global epoch only if
//! all currently pinned mutators have been pinned in the current epoch.
//!
//! If an object became garbage in some epoch, then we can be sure that after two advancements no
//! mutator will hold a reference to it. That is the crux of safe memory reclamation.

use std::ops::Deref;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};

use mutator::LocalEpoch;
use mutator::Scope;
use sync::list::{List, IterResult};
use crossbeam_utils::cache_padded::CachePadded;

/// The global epoch is a (cache-padded) integer.
#[derive(Default, Debug)]
pub struct Epoch {
    epoch: CachePadded<AtomicUsize>,
}

impl Epoch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempts to advance the global epoch.
    ///
    /// The global epoch can advance only if all currently pinned mutators have been pinned in the
    /// current epoch.
    ///
    /// Returns the current global epoch.
    ///
    /// `try_advance()` is annotated `#[cold]` because it is rarely called.
    #[cold]
    pub fn try_advance<'scope>(&self, registries: &List<LocalEpoch>, scope: &Scope) -> usize {
        let epoch = self.epoch.load(Relaxed);
        ::std::sync::atomic::fence(SeqCst);

        // Traverse the linked list of mutator registries.
        let mut registries = registries.iter(scope);
        loop {
            match registries.next() {
                IterResult::Abort => {
                    // We leave the job to the mutator that also tries to advance to epoch and
                    // continues to iterate the registries.
                    return epoch;
                }
                IterResult::None => break,
                IterResult::Some(local_epoch) => {
                    let (mutator_is_pinned, mutator_epoch) = local_epoch.get_state();

                    // If the mutator was pinned in a different epoch, we cannot advance the global
                    // epoch just yet.
                    if mutator_is_pinned && mutator_epoch != epoch {
                        return epoch;
                    }
                }
            }
        }
        ::std::sync::atomic::fence(Acquire);

        // All pinned mutators were pinned in the current global epoch.  Try advancing the epoch. We
        // increment by 2 and simply wrap around on overflow.
        let epoch_new = epoch.wrapping_add(2);
        self.epoch.store(epoch_new, Release);
        epoch_new
    }
}

impl Deref for Epoch {
    type Target = AtomicUsize;

    fn deref(&self) -> &Self::Target {
        &self.epoch
    }
}


#[cfg(test)]
mod tests {}
