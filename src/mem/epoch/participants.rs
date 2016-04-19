// Manages the global participant list, which is an intrustive list in
// which items are lazily removed on traversal (after being
// "logically" deleted by becoming inactive.)

use std::mem;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};

use mem::epoch::{Atomic, Owned, Guard};
use mem::epoch::participant::Participant;
use mem::CachePadded;

/// Global, threadsafe list of threads participating in epoch management.
#[derive(Debug)]
pub struct Participants {
    head: Atomic<ParticipantNode>
}

#[derive(Debug)]
pub struct ParticipantNode(CachePadded<Participant>);

impl ParticipantNode {
    pub fn new() -> ParticipantNode {
        ParticipantNode(CachePadded::new(Participant::new()))
    }
}

impl Deref for ParticipantNode {
    type Target = Participant;
    fn deref(&self) -> &Participant {
        &self.0
    }
}

impl DerefMut for ParticipantNode {
    fn deref_mut(&mut self) -> &mut Participant {
        &mut self.0
    }
}

impl Participants {
    #[cfg(not(feature = "nightly"))]
    pub fn new() -> Participants {
        Participants { head: Atomic::null() }
    }

    #[cfg(feature = "nightly")]
    pub const fn new() -> Participants {
        Participants { head: Atomic::null() }
    }

    /// Enroll a new thread in epoch management by adding a new `Particpant`
    /// record to the global list.
    pub fn enroll(&self) -> *const Participant {
        let mut participant = Owned::new(ParticipantNode::new());

        // we ultimately use epoch tracking to free Participant nodes, but we
        // can't actually enter an epoch here, so fake it; we know the node
        // can't be removed until marked inactive anyway.
        let fake_guard = ();
        let g: &'static Guard = unsafe { mem::transmute(&fake_guard) };
        loop {
            let head = self.head.load(Relaxed, g);
            participant.next.store_shared(head, Relaxed);
            match self.head.cas_and_ref(head, participant, Release, g) {
                Ok(shared) => {
                    let shared: &Participant = &shared;
                    return shared;
                }
                Err(owned) => {
                    participant = owned;
                }
            }
        }
    }

    pub fn iter<'a>(&'a self, g: &'a Guard) -> Iter<'a> {
        Iter {
            guard: g,
            next: &self.head,
            needs_acq: true,
        }
    }
}

#[derive(Debug)]
pub struct Iter<'a> {
    // pin to an epoch so that we can free inactive nodes
    guard: &'a Guard,
    next: &'a Atomic<ParticipantNode>,

    // an Acquire read is needed only for the first read, due to release
    // sequences
    needs_acq: bool,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        let mut cur = if self.needs_acq {
            self.needs_acq = false;
            self.next.load(Acquire, self.guard)
        } else {
            self.next.load(Relaxed, self.guard)
        };

        while let Some(n) = cur {
            // attempt to clean up inactive nodes
            if !n.active.load(Relaxed) {
                cur = n.next.load(Relaxed, self.guard);
                // TODO: actually reclaim inactive participants!
            } else {
                self.next = &n.next;
                return Some(&n)
            }
        }

        None
    }
}
