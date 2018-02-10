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
//! When a participant is pinned, a `Guard` is returned as a witness that the participant is pinned.
//! Guards are necessary for performing atomic operations, and for freeing/dropping locations.

use core::cell::{Cell, UnsafeCell};
use core::mem;
use core::num::Wrapping;
use core::ptr;
use core::sync::atomic;
use core::sync::atomic::Ordering;
use alloc::boxed::Box;
use alloc::arc::Arc;

use crossbeam_utils::cache_padded::CachePadded;
use nodrop::NoDrop;

use atomic::Owned;
use epoch::{AtomicEpoch, Epoch};
use guard::{unprotected, Guard};
use garbage::{Bag, Garbage};
use sync::list::{List, Entry, IterError, IsElement};
use sync::queue::Queue;

/// Number of bags to destroy.
const COLLECT_STEPS: usize = 8;

/// Number of pinnings after which a participant will execute some deferred functions from the
/// global queue.
const PINNINGS_BETWEEN_COLLECT: usize = 128;

/// The global data for a garbage collector.
pub struct Global {
    /// The intrusive linked list of `Local`s.
    locals: List<Local>,

    /// The global queue of bags of deferred functions.
    queue: Queue<(Epoch, Bag)>,

    /// The global epoch.
    epoch: CachePadded<AtomicEpoch>,
}

impl Global {
    /// Creates a new global data for garbage collection.
    #[inline]
    pub fn new() -> Global {
        Global {
            locals: List::new(),
            queue: Queue::new(),
            epoch: CachePadded::new(AtomicEpoch::new(Epoch::starting())),
        }
    }

    /// Returns the current global epoch.
    pub fn load_epoch(&self, ordering: Ordering) -> Epoch {
        self.epoch.load(ordering)
    }

    /// Pushes the bag into the global queue and replaces the bag with a new empty bag.
    pub fn push_bag(&self, bag: &mut Bag, guard: &Guard) {
        let bag = mem::replace(bag, Bag::new());

        atomic::fence(Ordering::SeqCst);

        let epoch = self.epoch.load(Ordering::Relaxed);
        self.queue.push((epoch, bag), guard);
    }

    /// Collects several bags from the global queue and executes deferred functions in them.
    ///
    /// Note: This may itself produce garbage and in turn allocate new bags.
    ///
    /// `pin()` rarely calls `collect()`, so we want the compiler to place that call on a cold
    /// path. In other words, we want the compiler to optimize branching for the case when
    /// `collect()` is not called.
    #[cold]
    pub fn collect(&self, guard: &Guard) {
        let global_epoch = self.try_advance(guard);

        let condition = |item: &(Epoch, Bag)| {
            // A pinned participant can witness at most one epoch advancement. Therefore, any bag
            // that is within one epoch of the current one cannot be destroyed yet.
            global_epoch.wrapping_sub(item.0) >= 2
        };

        let steps = if cfg!(feature = "sanitize") {
            usize::max_value()
        } else {
            COLLECT_STEPS
        };

        for _ in 0..steps {
            match self.queue.try_pop_if(&condition, guard) {
                None => break,
                Some(bag) => drop(bag),
            }
        }
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
    pub fn try_advance(&self, guard: &Guard) -> Epoch {
        let global_epoch = self.epoch.load(Ordering::Relaxed);
        atomic::fence(Ordering::SeqCst);

        // TODO(stjepang): `Local`s are stored in a linked list because linked lists are fairly
        // easy to implement in a lock-free manner. However, traversal can be slow due to cache
        // misses and data dependencies. We should experiment with other data structures as well.
        for local in self.locals.iter(&guard) {
            match local {
                Err(IterError::Stalled) => {
                    // A concurrent thread stalled this iteration. That thread might also try to
                    // advance the epoch, in which case we leave the job to it. Otherwise, the
                    // epoch will not be advanced.
                    return global_epoch;
                }
                Ok(local) => {
                    let local_epoch = local.epoch.load(Ordering::Relaxed);

                    // If the participant was pinned in a different epoch, we cannot advance the
                    // global epoch just yet.
                    if local_epoch.is_pinned() && local_epoch.unpinned() != global_epoch {
                        return global_epoch;
                    }
                }
            }
        }
        atomic::fence(Ordering::Acquire);

        // All pinned participants were pinned in the current global epoch.
        // Now let's advance the global epoch...
        //
        // Note that if another thread already advanced it before us, this store will simply
        // overwrite the global epoch with the same value. This is true because `try_advance` was
        // called from a thread that was pinned in `global_epoch`, and the global epoch cannot be
        // advanced two steps ahead of it.
        let new_epoch = global_epoch.successor();
        self.epoch.store(new_epoch, Ordering::Release);
        new_epoch
    }
}

/// Participant for garbage collection.
pub struct Local {
    /// A node in the intrusive linked list of `Local`s.
    entry: Entry,

    /// The local epoch.
    epoch: AtomicEpoch,

    /// A reference to the global data.
    ///
    /// When all guards and handles get dropped, this reference is destroyed.
    global: UnsafeCell<NoDrop<Arc<Global>>>,

    /// The local bag of deferred functions.
    bag: UnsafeCell<Bag>,

    /// The number of guards keeping this participant pinned.
    guard_count: Cell<usize>,

    /// The number of active handles.
    handle_count: Cell<usize>,

    /// Total number of pinnings performed.
    ///
    /// This is just an auxilliary counter that sometimes kicks off collection.
    pin_count: Cell<Wrapping<usize>>,
}

unsafe impl Sync for Local {}

impl Local {
    /// Registers a new `Local` in the provided `Global`.
    pub fn register(global: &Arc<Global>) -> *const Local {
        unsafe {
            // Since we dereference no pointers in this block, it is safe to use `unprotected`.

            let local = Owned::new(Local {
                entry: Entry::default(),
                epoch: AtomicEpoch::new(Epoch::starting()),
                global: UnsafeCell::new(NoDrop::new(global.clone())),
                bag: UnsafeCell::new(Bag::new()),
                guard_count: Cell::new(0),
                handle_count: Cell::new(1),
                pin_count: Cell::new(Wrapping(0)),
            }).into_shared(&unprotected());
            global.locals.insert(local, &unprotected());
            local.as_raw()
        }
    }

    /// Returns whether the local garbage bag is empty.
    #[inline]
    pub fn is_bag_empty(&self) -> bool {
        unsafe { (*self.bag.get()).is_empty() }
    }

    /// Returns a reference to the `Global` in which this `Local` resides.
    #[inline]
    pub fn global(&self) -> &Global {
        unsafe { &*self.global.get() }
    }

    /// Returns `true` if the current participant is pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.guard_count.get() > 0
    }

    pub fn defer(&self, mut garbage: Garbage, guard: &Guard) {
        let bag = unsafe { &mut *self.bag.get() };

        while let Err(g) = bag.try_push(garbage) {
            self.global().push_bag(bag, guard);
            garbage = g;
        }
    }

    pub fn flush(&self, guard: &Guard) {
        let bag = unsafe { &mut *self.bag.get() };

        if !bag.is_empty() {
            self.global().push_bag(bag, guard);
        }

        self.global().collect(guard);
    }

    /// Pins the `Local`.
    #[inline]
    pub fn pin(&self) -> Guard {
        let guard = unsafe { Guard::new(self) };

        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count.checked_add(1).unwrap());

        if guard_count == 0 {
            let global_epoch = self.global().epoch.load(Ordering::Relaxed);
            let new_epoch = global_epoch.pinned();

            // Now we must store `new_epoch` into `self.epoch` and execute a `SeqCst` fence.
            // The fence makes sure that any future loads from `Atomic`s will not happen before
            // this store.
            if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
                // HACK(stjepang): On x86 architectures there are two different ways of executing
                // a `SeqCst` fence.
                //
                // 1. `atomic::fence(SeqCst)`, which compiles into a `mfence` instruction.
                // 2. `_.compare_and_swap(_, _, SeqCst)`, which compiles into a `lock cmpxchg`
                //    instruction.
                //
                // Both instructions have the effect of a full barrier, but benchmarks have shown
                // that the second one makes pinning faster in this particular case.
                let current = Epoch::starting();
                let previous = self.epoch.compare_and_swap(current, new_epoch, Ordering::SeqCst);
                debug_assert_eq!(current, previous, "participant was expected to be unpinned");
            } else {
                self.epoch.store(new_epoch, Ordering::Relaxed);
                atomic::fence(Ordering::SeqCst);
            }

            // Increment the pin counter.
            let count = self.pin_count.get();
            self.pin_count.set(count + Wrapping(1));

            // After every `PINNINGS_BETWEEN_COLLECT` try advancing the epoch and collecting
            // some garbage.
            if count.0 % PINNINGS_BETWEEN_COLLECT == 0 {
                self.global().collect(&guard);
            }
        }

        guard
    }

    /// Unpins the `Local`.
    #[inline]
    pub fn unpin(&self) {
        let guard_count = self.guard_count.get();
        self.guard_count.set(guard_count - 1);

        if guard_count == 1 {
            self.epoch.store(Epoch::starting(), Ordering::Release);

            if self.handle_count.get() == 0 {
                self.finalize();
            }
        }
    }

    /// Unpins and then pins the `Local`.
    #[inline]
    pub fn repin(&self) {
        let guard_count = self.guard_count.get();

        // Update the local epoch only if there's only one guard.
        if guard_count == 1 {
            let epoch = self.epoch.load(Ordering::Relaxed);
            let global_epoch = self.global().epoch.load(Ordering::Relaxed);

            // Update the local epoch only if the global epoch is greater than the local epoch.
            if epoch != global_epoch {
                // We store the new epoch with `Release` because we need to ensure any memory
                // accesses from the previous epoch do not leak into the new one.
                self.epoch.store(global_epoch, Ordering::Release);

                // However, we don't need a following `SeqCst` fence, because it is safe for memory
                // accesses from the new epoch to be executed before updating the local epoch.  At
                // worse, other threads will see the new epoch late and delay GC slightly.
            }
        }
    }

    /// Increments the handle count.
    #[inline]
    pub fn acquire_handle(&self) {
        let handle_count = self.handle_count.get();
        debug_assert!(handle_count >= 1);
        self.handle_count.set(handle_count + 1);
    }

    /// Decrements the handle count.
    #[inline]
    pub fn release_handle(&self) {
        let guard_count = self.guard_count.get();
        let handle_count = self.handle_count.get();
        debug_assert!(handle_count >= 1);
        self.handle_count.set(handle_count - 1);

        if guard_count == 0 && handle_count == 1 {
            self.finalize();
        }
    }

    /// Removes the `Local` from the global linked list.
    #[cold]
    fn finalize(&self) {
        debug_assert_eq!(self.guard_count.get(), 0);
        debug_assert_eq!(self.handle_count.get(), 0);

        // Temporarily increment handle count. This is required so that the following call to `pin`
        // doesn't call `finalize` again.
        self.handle_count.set(1);
        unsafe {
            // Pin and move the local bag into the global queue. It's important that `push_bag`
            // doesn't defer destruction on any new garbage.
            let guard = &self.pin();
            self.global().push_bag(&mut *self.bag.get(), guard);
        }
        // Revert the handle count back to zero.
        self.handle_count.set(0);

        unsafe {
            // Take the reference to the `Global` out of this `Local`. Since we're not protected
            // by a guard at this time, it's crucial that the reference is read before marking the
            // `Local` as deleted.
            let global: Arc<Global> = ptr::read(&**self.global.get());

            // Mark this node in the linked list as deleted.
            self.entry.delete(&unprotected());

            // Finally, drop the reference to the global.  Note that this might be the last
            // reference to the `Global`. If so, the global data will be destroyed and all deferred
            // functions in its queue will be executed.
            drop(global);
        }
    }
}

impl IsElement<Local> for Local {
    fn entry_of(local: &Local) -> &Entry {
        let entry_ptr = (local as *const Local as usize + offset_of!(Local, entry)) as *const Entry;
        unsafe { &*entry_ptr }
    }

    unsafe fn element_of(entry: &Entry) -> &Local {
        // offset_of! macro uses unsafe, but it's unnecessary in this context.
        #[allow(unused_unsafe)]
        let local_ptr = (entry as *const Entry as usize - offset_of!(Local, entry)) as *const Local;
        &*local_ptr
    }

    unsafe fn finalize(entry: &Entry) {
        let local = Self::element_of(entry);
        drop(Box::from_raw(local as *const Local as *mut Local));
    }
}
