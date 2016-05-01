// Manages the global participant list, which is an intrustive list in
// which items are lazily removed on traversal (after being
// "logically" deleted by becoming inactive.)

use std::mem;
use std::ptr;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};

use mem::epoch::{MarkedAtomic, Owned, Shared, Guard};
use mem::epoch::participant::Participant;
use mem::CachePadded;

/// Global, threadsafe list of threads participating in epoch management.
#[derive(Debug)]
pub struct Participants {
    head: MarkedAtomic<ParticipantNode>
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
        Participants { head: MarkedAtomic::null() }
    }

    #[cfg(feature = "nightly")]
    pub const fn new() -> Participants {
        Participants { head: MarkedAtomic::null() }
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
            let head = self.head.load(Relaxed, g).0;
            participant.next.store_shared(head, 0, Relaxed);
            match self.head.cas_and_ref(head, 0, participant, 0, Release, g) {
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
            prev: &self.head,
            current: self.head.load(Acquire, g).0
        }
    }
}

#[derive(Debug)]
pub struct Iter<'a> {
    // pin to an epoch so that we can free inactive nodes
    guard: &'a Guard,
    prev: &'a MarkedAtomic<ParticipantNode>,
    current: Option<Shared<'a, ParticipantNode>>
}

fn opt_shared_into_raw<'a, T>(val: Option<Shared<'a, T>>) -> *mut T {
    val.map_or(ptr::null_mut(), |p| p.as_raw())
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        // In the simple case, there is nothing to clean up
        let mut cur = match self.current {
            None => return None,
            Some(node) => {
                let (next, mark) = node.next.load(Relaxed, self.guard);
                if mark == 0 {
                    self.prev = &node.next;
                    self.current = next;
                    return Some(&*node);
                }
                next
            }
        };

        // Scan for an active node
        let mut final_next = None;
        while let Some(node) = cur {
            let (next, mark) = node.next.load(Relaxed, self.guard);
            if mark == 0 {
                final_next = Some(next);
                break;
            } else {
                cur = next;
            }
        }

        // Try to clean up the inactive participants. If we fail, someone else will
        // deal with it.
        if self.prev.cas_shared(self.current, 0, cur, 0, Relaxed) {
            let mut to_cleanup = self.current;
            while opt_shared_into_raw(to_cleanup) != opt_shared_into_raw(cur) {
                let node = to_cleanup.unwrap();
                to_cleanup = node.next.load(Relaxed, self.guard).0;
                unsafe { self.guard.unlinked(node); }
            }
        }

        if let Some(node) = cur {
            self.prev = &node.next;
            self.current = final_next.unwrap();
        }
        cur.map(|n| &***n)
    }
}
