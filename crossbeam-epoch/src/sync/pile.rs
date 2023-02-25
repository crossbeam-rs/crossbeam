use crate::primitive::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;

use crossbeam_utils::CachePadded;

use crate::deferred::Deferred;
use crate::primitive::sync::atomic::{AtomicPtr, AtomicUsize};

pub(crate) enum PushResult {
    Done,
    ShouldRealloc,
}

/// An array of [Deferred]
pub(crate) struct Segment {
    pub(crate) len: CachePadded<AtomicUsize>,
    pub(crate) deferreds: Box<[MaybeUninit<UnsafeCell<Deferred>>]>,
}

// For some reason using Owned<> for this gives stacked borrows errors
pub(crate) fn uninit_boxed_slice<T>(len: usize) -> Box<[MaybeUninit<T>]> {
    unsafe {
        let ptr = if len == 0 {
            core::mem::align_of::<T>() as *mut MaybeUninit<T>
        } else {
            alloc::alloc::alloc(alloc::alloc::Layout::array::<T>(len).unwrap()) as _
        };
        Box::from_raw(core::slice::from_raw_parts_mut(ptr, len))
    }
}

impl Segment {
    #[cfg(crossbeam_loom)]
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        let mut deferreds = uninit_boxed_slice(capacity);
        // Work around lack of UnsafeCell::raw_get (1.56)
        deferreds.iter_mut().for_each(|uninit| {
            uninit.write(UnsafeCell::new(Deferred::NO_OP));
        });
        Self {
            len: CachePadded::new(AtomicUsize::new(0)),
            deferreds,
        }
    }
    #[cfg(not(crossbeam_loom))]
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            len: CachePadded::new(AtomicUsize::new(0)),
            deferreds: uninit_boxed_slice(capacity),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.deferreds.len()
    }

    /// # Safety:
    /// This must not be called concurrently with [call()].
    pub(crate) unsafe fn try_push(&self, deferred: &mut Vec<Deferred>) -> PushResult {
        let slot = self.len.fetch_add(deferred.len(), Ordering::Relaxed);
        if slot >= self.capacity() {
            return PushResult::Done;
        }
        let end = slot + deferred.len();
        let not_pushed = end.saturating_sub(self.capacity());
        for (slot, deferred) in self.deferreds[slot..]
            .iter()
            .zip(deferred.drain(not_pushed..))
        {
            // Work around lack of UnsafeCell::raw_get (1.56)
            #[cfg(crossbeam_loom)]
            slot.assume_init_ref().with_mut(|slot| slot.write(deferred));
            #[cfg(not(crossbeam_loom))]
            core::ptr::write(slot.as_ptr() as *mut _, deferred);
        }
        if end >= self.capacity() {
            PushResult::ShouldRealloc
        } else {
            PushResult::Done
        }
    }

    /// # Safety:
    /// This must not be called concurrently with [try_push()] or [call()].
    pub(crate) unsafe fn call(&self) {
        let end = self.capacity().min(self.len.load(Ordering::Relaxed));
        for deferred in &self.deferreds[..end] {
            deferred.assume_init_read().with(|ptr| ptr.read().call())
        }
        self.len.store(0, Ordering::Relaxed);
    }
}

impl Drop for Segment {
    fn drop(&mut self) {
        unsafe { self.call() }
    }
}

/// A stack of garbage.
pub(crate) struct Pile {
    /// Stashed objects.
    pub(crate) current: AtomicPtr<Segment>,
}

impl Pile {
    /// # Safety:
    /// This must not be called concurrently with [call()].
    pub(crate) unsafe fn try_push(&self, deferred: &mut Vec<Deferred>) {
        let segment = self.current.load(Ordering::Acquire);
        if let PushResult::ShouldRealloc = (*segment).try_push(deferred) {
            let next = Box::into_raw(Box::new(Segment::new((*segment).capacity() * 2)));
            // Synchronize initialization of the Segment
            self.current.store(next, Ordering::Release);
            deferred.push(Deferred::new(move || drop(Box::from_raw(segment))));
            // We're potentially holding a lot of garbage now, make an attempt to get rid of it sooner
            self.try_push(deferred)
        }
    }

    /// # Safety:
    /// This must not be called concurrently with [try_push()] or [call()].
    pub(crate) unsafe fn call(&self) {
        (*self.current.load(Ordering::Relaxed)).call()
    }
}

impl Default for Pile {
    fn default() -> Self {
        Pile {
            current: AtomicPtr::new(Box::into_raw(Box::new(Segment::new(64)))),
        }
    }
}

impl Drop for Pile {
    fn drop(&mut self) {
        unsafe {
            // Ordering is taken care of because `Global` is behind an `Arc`
            drop(Box::from_raw(self.current.load(Ordering::Relaxed)));
        }
    }
}

impl fmt::Debug for Pile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pile").finish_non_exhaustive()
    }
}

#[cfg(all(test, not(crossbeam_loom)))]
mod tests {
    use super::*;

    #[test]
    fn check_bag() {
        static FLAG: AtomicUsize = AtomicUsize::new(0);
        fn incr() {
            FLAG.fetch_add(1, Ordering::Relaxed);
        }

        let bag = Pile::default();
        let mut buffer1 = (0..3).map(|_| Deferred::new(incr)).collect();
        let mut buffer2 = (0..18).map(|_| Deferred::new(incr)).collect();

        unsafe { bag.try_push(&mut buffer1) };
        assert_eq!(buffer1.len(), 0);
        assert_eq!(FLAG.load(Ordering::Relaxed), 0);
        unsafe { bag.call() };
        assert_eq!(FLAG.load(Ordering::Relaxed), 3);
        unsafe { bag.try_push(&mut buffer2) };
        assert_eq!(buffer2.len(), 0);
        assert_eq!(FLAG.load(Ordering::Relaxed), 3);
        drop(bag);
        assert_eq!(FLAG.load(Ordering::Relaxed), 21);
    }
}
