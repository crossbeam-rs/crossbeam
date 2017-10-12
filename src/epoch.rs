//! The global epoch
//!
//! The last bit in this number is unused and is always zero. Every so often the global epoch is
//! incremented, i.e. we say it "advances". A pinned participant may advance the global epoch only
//! if all currently pinned participants have been pinned in the current epoch.
//!
//! If an object became garbage in some epoch, then we can be sure that after two advancements no
//! participant will hold a reference to it. That is the crux of safe memory reclamation.

use std::ops::Deref;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};

use internal::LocalEpoch;
use scope::Scope;
use sync::list::{List, IterError};
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
    /// The global epoch can advance only if all currently pinned participants have been pinned in
    /// the current epoch.
    ///
    /// Returns the current global epoch.
    ///
    /// `try_advance()` is annotated `#[cold]` because it is rarely called.
    #[cold]
    pub fn try_advance(&self, registries: &List<LocalEpoch>, scope: &Scope) -> usize {
        let epoch = self.epoch.load(Relaxed);
        ::std::sync::atomic::fence(SeqCst);

        // Traverse the linked list of participant registries.
        for participant in registries.iter(scope) {
            match participant {
                Err(IterError::LostRace) => {
                    // We leave the job to the participant that won the race, which continues to
                    // iterate the registries and tries to advance to epoch.
                    return epoch;
                }
                Ok(local_epoch) => {
                    let local_epoch = local_epoch.get();
                    let (participant_is_pinned, participant_epoch) = local_epoch.get_state();

                    // If the participant was pinned in a different epoch, we cannot advance the
                    // global epoch just yet.
                    if participant_is_pinned && participant_epoch != epoch {
                        return epoch;
                    }
                }
            }
        }
        ::std::sync::atomic::fence(Acquire);

        // All pinned participants were pinned in the current global epoch.  Try advancing the
        // epoch. We increment by 2 and simply wrap around on overflow.
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
