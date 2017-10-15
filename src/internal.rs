//! The global data and participant for garbage collection.
//!
//! # Registration
//!
//! In order to track all participants in one place, we need some form of participant
//! registration. When a participant is created, it is registered to a global lock-free
//! singly-linked list of registries; and when a participant is leaving, it is unregistered from the
//! list.
//!
//! # Pinning
//!
//! Every participant contains an integer that tells whether the participant is pinned and if so,
//! what was the global epoch at the time it was pinned. Participants also hold a pin counter that
//! aids in periodic global epoch advancement.
//!
//! When a participant is pinned, a `Scope` is returned as a witness that the participant is pinned.
//! Scopes are necessary for performing atomic operations, and for freeing/dropping locations.
//!
//! # Example
//!
//! `Global` is the global data for a garbage collector, and `Local` is a participant of a garbage
//! collector. Use `Global` and `Local` when you want to embed a garbage collector in another
//! systems library, e.g. memory allocator or thread manager.
//!
//! ```ignore
//! let global = Global::new();
//! let local = Local::new(&global);
//! unsafe {
//!     local.pin(&global, |scope| {
//!         scope.flush();
//!     });
//! }
//! ```

use std::cell::{Cell, UnsafeCell};
use std::cmp;
use std::sync::atomic::{AtomicUsize, Ordering};
use scope::{Scope, unprotected};
use garbage::Bag;
use epoch::Epoch;
use sync::list::{List, Node};
use sync::queue::Queue;


/// The global data for a garbage collector.
#[derive(Debug)]
pub struct Global {
    /// The head pointer of the list of participant registries.
    registries: List<LocalEpoch>,
    /// A reference to the global queue of garbages.
    garbages: Queue<(usize, Bag)>,
    /// A reference to the global epoch.
    epoch: Epoch,
}

impl Global {
    /// Number of bags to destroy.
    const COLLECT_STEPS: usize = 8;

    /// Creates a new global data for garbage collection.
    #[inline]
    pub fn new() -> Self {
        Self {
            registries: List::new(),
            garbages: Queue::new(),
            epoch: Epoch::new(),
        }
    }

    /// Returns the global epoch.
    #[inline]
    pub fn get_epoch(&self) -> usize {
        self.epoch.load(Ordering::Relaxed)
    }

    /// Pushes the bag onto the global queue and replaces the bag with a new empty bag.
    pub fn push_bag(&self, bag: &mut Bag, scope: &Scope) {
        let epoch = self.epoch.load(Ordering::Relaxed);
        let bag = ::std::mem::replace(bag, Bag::new());
        ::std::sync::atomic::fence(Ordering::SeqCst);
        self.garbages.push((epoch, bag), scope);
    }

    /// Collect several bags from the global garbage queue and destroy their objects.
    ///
    /// Note: This may itself produce garbage and in turn allocate new bags.
    ///
    /// `pin()` rarely calls `collect()`, so we want the compiler to place that call on a cold
    /// path. In other words, we want the compiler to optimize branching for the case when
    /// `collect()` is not called.
    #[cold]
    pub fn collect(&self, scope: &Scope) {
        let epoch = self.epoch.try_advance(&self.registries, scope);

        let condition = |bag: &(usize, Bag)| {
            // A pinned participant can witness at most one epoch advancement. Therefore, any bag
            // that is within one epoch of the current one cannot be destroyed yet.
            let diff = epoch.wrapping_sub(bag.0);
            cmp::min(diff, 0usize.wrapping_sub(diff)) > 2
        };

        for _ in 0..Self::COLLECT_STEPS {
            match self.garbages.try_pop_if(&condition, scope) {
                None => break,
                Some(bag) => drop(bag),
            }
        }
    }
}

// FIXME(stjepang): Registries are stored in a linked list because linked lists are fairly easy to
// implement in a lock-free manner. However, traversal is rather slow due to cache misses and data
// dependencies. We should experiment with other data structures as well.
/// Participant for garbage collection
#[derive(Debug)]
pub struct Local {
    /// The local garbage objects that will be later freed.
    bag: UnsafeCell<Bag>,
    /// This participant's entry in the local epoch list.  It points to a node in `Global`, so it is
    /// alive as far as the `Global` is alive.
    local_epoch: *const Node<LocalEpoch>,
    /// Whether the participant is currently pinned.
    is_pinned: Cell<bool>,
    /// Total number of pinnings performed.
    pin_count: Cell<usize>,
}

/// An entry in the linked list of the registered participants.
#[derive(Default, Debug)]
pub struct LocalEpoch {
    /// The least significant bit is set if the participant is currently pinned. The rest of the
    /// bits encode the current epoch.
    state: AtomicUsize,
}

impl Local {
    /// Number of pinnings after which a participant will collect some global garbage.
    const PINS_BETWEEN_COLLECT: usize = 128;

    /// Creates a participant to the garbage collection global data.
    #[inline]
    pub fn new(global: &Global) -> Self {
        let local_epoch = unsafe {
            // Since we dereference no pointers in this block, it is safe to use `unprotected`.
            unprotected(|scope| {
                global.registries.insert(LocalEpoch::new(), scope).as_raw()
            })
        };

        Self {
            bag: UnsafeCell::new(Bag::new()),
            local_epoch,
            is_pinned: Cell::new(false),
            pin_count: Cell::new(0),
        }
    }

    /// Pins the current participant, executes a function, and unpins the participant.
    ///
    /// The provided function takes a `Scope`, which can be used to interact with [`Atomic`]s. The
    /// scope serves as a proof that whatever data you load from an [`Atomic`] will not be
    /// concurrently deleted by another participant while the scope is alive.
    ///
    /// Note that keeping a participant pinned for a long time prevents memory reclamation of any
    /// newly deleted objects protected by [`Atomic`]s. The provided function should be very quick -
    /// generally speaking, it shouldn't take more than 100 ms.
    ///
    /// Pinning is reentrant. There is no harm in pinning a participant while it's already pinned
    /// (repinning is essentially a noop).
    ///
    /// Pinning itself comes with a price: it begins with a `SeqCst` fence and performs a few other
    /// atomic operations. However, this mechanism is designed to be as performant as possible, so
    /// it can be used pretty liberally. On a modern machine pinning takes 10 to 15 nanoseconds.
    ///
    /// # Safety
    ///
    /// You should pass `global` that is used to create this `Local`. Otherwise, the behavior is
    /// undefined.
    ///
    /// [`Atomic`]: struct.Atomic.html
    pub unsafe fn pin<F, R>(&self, global: &Global, f: F) -> R
    where
        F: FnOnce(&Scope) -> R,
    {
        let local_epoch = (*self.local_epoch).get();
        let scope = Scope {
            global,
            bag: self.bag.get(),
        };

        let was_pinned = self.is_pinned.get();
        if !was_pinned {
            // Increment the pin counter.
            let count = self.pin_count.get();
            self.pin_count.set(count.wrapping_add(1));

            // Pin the participant.
            self.is_pinned.set(true);
            let epoch = global.get_epoch();
            local_epoch.set_pinned(epoch);

            // If the counter progressed enough, try advancing the epoch and collecting garbage.
            if count % Self::PINS_BETWEEN_COLLECT == 0 {
                global.collect(&scope);
            }
        }

        // This will unpin the participant even if `f` panics.
        defer! {
            if !was_pinned {
                // Unpin the participant.
                local_epoch.set_unpinned();
                self.is_pinned.set(false);
            }
        }

        f(&scope)
    }

    /// Returns `true` if the current participant is pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.is_pinned.get()
    }

    /// Unregisters itself from the garbage collector.
    ///
    /// # Safety
    ///
    /// You should pass `global` that is used to create this `Local`. Also, a `Local` should be
    /// unregistered once, and after it is unregistered it should not be `pin()`ned. Otherwise, the
    /// behavior is undefined.
    pub unsafe fn unregister(&self, global: &Global) {
        // Now that the participant is exiting, we must move the local bag into the global garbage
        // queue. Also, let's try advancing the epoch and help free some garbage.

        self.pin(global, |scope| {
            // Spare some cycles on garbage collection.
            global.collect(scope);

            // Unregister the participant by marking this entry as deleted.
            (*self.local_epoch).delete(scope);

            // Push the local bag into the global garbage queue.
            global.push_bag(&mut *self.bag.get(), scope);
        });
    }
}

impl LocalEpoch {
    /// Creates a new local epoch.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns if the participant is pinned, and if so, the epoch at which it is pinned.
    #[inline]
    pub fn get_state(&self) -> (bool, usize) {
        let state = self.state.load(Ordering::Relaxed);
        ((state & 1) == 1, state & !1)
    }

    /// Marks the participant as pinned.
    ///
    /// Must not be called if the participant is already pinned!
    #[inline]
    pub fn set_pinned(&self, epoch: usize) {
        let state = epoch | 1;

        // Now we must store `state` into `self.state`. It's important that any succeeding loads
        // don't get reordered with this store. In order words, this participant's epoch must be
        // fully announced to other participants. Only then it becomes safe to load from the shared
        // memory.
        if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
            // On x86 architectures we have a choice:
            // 1. `atomic::fence(SeqCst)`, which compiles to a `mfence` instruction.
            // 2. `compare_and_swap(_, _, SeqCst)`, which compiles to a `lock cmpxchg` instruction.
            //
            // Both instructions have the effect of a full barrier, but the second one seems to be
            // faster in this particular case.
            let result = self.state.compare_and_swap(0, state, Ordering::SeqCst);
            debug_assert_eq!(0, result, "LocalEpoch::set_pinned()'s CAS should succeed.");
        } else {
            self.state.store(state, Ordering::Relaxed);
            ::std::sync::atomic::fence(Ordering::SeqCst);
        }
    }

    /// Marks the participant as unpinned.
    #[inline]
    pub fn set_unpinned(&self) {
        // Clear the last bit.
        // We don't need to preserve the epoch, so just store the number zero.
        self.state.store(0, Ordering::Release);
    }
}
