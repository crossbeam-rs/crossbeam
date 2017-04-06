// Global participant list.
//
// The global participant list is an intrustive list in which items are lazily removed on traversal
// (after being "logically" deleted by becoming inactive).

use std::{ops, mem};
use std::sync::atomic;

use epoch::{Atomic, Guard};
use epoch::participant::Participant;
use CachePadded;

/// A participant node.
///
/// The data is cache padded to avoid cache line racing.
#[derive(Debug)]
pub struct ParticipantNode(CachePadded<Participant>);

impl Default for ParticipantNode {
    fn default() -> ParticipantNode {
        ParticipantNode(CachePadded::new(Participant::default()))
    }
}

impl ops::Deref for ParticipantNode {
    type Target = Participant;
    fn deref(&self) -> &Participant {
        &self.0
    }
}

impl ops::DerefMut for ParticipantNode {
    fn deref_mut(&mut self) -> &mut Participant {
        &mut self.0
    }
}

/// A thread-safe list of threads participating in epoch management.
///
/// Currently implemented like a Treiber stack.
#[derive(Debug, Default)]
pub struct Participants {
    /// The head node.
    head: Atomic<ParticipantNode>,
}

impl Participants {
    // TODO: Remove this cfg when `const fn` is stabilized.
    /// Create an empty list of participants.
    #[cfg(feature = "nightly")]
    pub const fn new() -> Participants {
        Participants { head: Atomic::null() }
    }

    /// Enroll the current thread into the list.
    ///
    /// This adds the current thread to the list in epoch management by adding a new `Particpant`
    /// record to the global list.
    pub fn enroll(&self) -> *const Participant {
        // The new list.
        let mut participant = Box::new(ParticipantNode::default());

        // We ultimately use epoch tracking to free `Participant` nodes, but we can't actually
        // enter an epoch here, so fake it; we know the node can't be removed until marked inactive
        // anyway.
        let fake_guard = ();
        let guard: &'static Guard = unsafe { mem::transmute(&fake_guard) };

        loop {
            // Load the head of the participant list we're traversing.
            let head = self.head.load(atomic::Ordering::Relaxed, guard);
            // Move the head to the tail of the new list, which we will store.
            participant.next.store_shared(head, atomic::Ordering::Relaxed);

            // To solve the ABA problem, we must use CAS, testing against the loaded head.
            match self.head.compare_and_set_ref(head, participant, atomic::Ordering::Release, guard) {
                Ok(shared) => {
                    // It succeeded and the new list is now placed at `self`.

                    // We cast to `Participant` when we return.
                    // FIXME: This is an ugly ugly hack.
                    let shared: &Participant = &shared;
                    return shared;
                }
                Err(owned) => {
                    // It failed. Put back the value and retry.
                    participant = owned;
                }
            }
        }
    }

    /// Get an iterator over the participants.
    // FIXME: For some reason, removing these lifetimes breaks the code.
    pub fn iter<'a>(&'a self, g: &'a Guard) -> Iter<'a> {
        Iter {
            guard: g,
            next: &self.head,
            first: true,
        }
    }
}

/// An iterator over items of a participant list.
#[derive(Debug)]
pub struct Iter<'a> {
    // Pin to an epoch.
    //
    // This ensures that we can free inactive nodes.
    guard: &'a Guard,
    /// The next node.
    next: &'a Atomic<ParticipantNode>,
    // Has `self.next()` **not** been called before?
    first: bool,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        // Load the next node.
        let mut cur = self.next.load(if self.first {
            // Now that the method has been called, it should no longer be `true`.
            self.first = false;
            // An `Acquire` read is needed only for the first read, due to release sequences.
            atomic::Ordering::Acquire
        } else { atomic::Ordering::Relaxed }, self.guard);

        // Find the next available. node
        while let Some(n) = cur {
            // Attempt to clean up inactive nodes.
            if !n.active.load(atomic::Ordering::Relaxed) {
                // Go to the next node and repeat.
                cur = n.next.load(atomic::Ordering::Relaxed, self.guard);

                // TODO: Actually reclaim inactive participants!
            } else {
                // Go to the next node.
                self.next = &n.next;

                return Some(&n);
            }
        }

        // If the loop was finished without returning, no more nodes are to be found.
        None
    }
}
