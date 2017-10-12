use std::mem;
use std::ptr;

use garbage::{Garbage, Bag};
use internal::Global;

/// A witness that the current participant is pinned.
///
/// A `&Scope` is a witness that the current participant is pinned. Lots of methods that interact
/// with [`Atomic`]s can safely be called only while the participant is pinned so they often require
/// a `&Scope`.
///
/// This data type is inherently bound to the thread that created it, therefore it does not
/// implement `Send` nor `Sync`.
///
/// [`Atomic`]: struct.Atomic.html
#[derive(Debug)]
pub struct Scope {
    /// A reference to the global data.
    pub(crate) global: *const Global,
    /// A reference to the thread-local bag.
    pub(crate) bag: *mut Bag, // !Send + !Sync
}

impl Scope {
    unsafe fn defer_garbage(&self, mut garbage: Garbage) {
        self.global.as_ref().map(|global| {
            let bag = &mut *self.bag;
            while let Err(g) = bag.try_push(garbage) {
                global.push_bag(bag, self);
                garbage = g;
            }
        });
    }

    /// Deferred execution of an arbitrary function `f`.
    ///
    /// This function inserts the function into a thread-local [`Bag`]. When the bag becomes full,
    /// the bag is flushed into the globally shared queue of bags.
    ///
    /// If this function is destroying a particularly large object, it is wise to follow up with a
    /// call to [`flush`] so that it doesn't get stuck waiting in the thread-local bag for a long
    /// time.
    ///
    /// [`Bag`]: struct.Bag.html
    /// [`flush`]: fn.flush.html
    pub unsafe fn defer<R, F: FnOnce() -> R + Send>(&self, f: F) {
        self.defer_garbage(Garbage::new(|| drop(f())))
    }

    /// Flushes all garbage in the thread-local storage into the global garbage queue, attempts to
    /// advance the epoch, and collects some garbage.
    ///
    /// Even though flushing can be explicitly called, it is also automatically triggered when the
    /// thread-local storage fills up or when we pin the current participant a specific number of
    /// times.
    ///
    /// It is wise to flush the bag just after `defer`ring the deallocation of a very large object,
    /// so that it isn't sitting in the thread-local bag for a long time.
    ///
    /// [`defer`]: fn.defer.html
    pub fn flush(&self) {
        unsafe {
            self.global.as_ref().map(|global| {
                let bag = &mut *self.bag;

                if !bag.is_empty() {
                    global.push_bag(bag, &self);
                }

                global.collect(&self);
            });
        }
    }
}

/// Returns a [`Scope`] without pinning any participant.
///
/// Sometimes, we'd like to have longer-lived scopes in which we know our thread can access atomics
/// without protection. This is true e.g. when deallocating a big data structure, or when
/// constructing it from a long iterator. In such cases we don't need to be overprotective because
/// there is no fear of other threads concurrently destroying objects.
///
/// Function `unprotected` is *unsafe* because we must promise that (1) the locations that we access
/// should not be deallocated by concurrent participants, and (2) the locations that we deallocate
/// should not be accessed by concurrent participants.
///
/// Just like with the safe epoch::pin function, unprotected use of atomics is enclosed within a
/// scope so that pointers created within it don't leak out or get mixed with pointers from other
/// scopes.
#[inline]
pub unsafe fn unprotected<F, R>(f: F) -> R
where
    F: FnOnce(&Scope) -> R,
{
    let scope = Scope {
        global: ptr::null(),
        bag: mem::uninitialized(),
    };
    f(&scope)
}
