//! Atomic storage and retrieval of an `Arc<T>`.

use std::{marker, mem};
use std::sync::Arc;
use std::sync::atomic::{self, AtomicUsize, AtomicPtr};

/// A thread-safe, reference-counted, and mutable container.
///
/// This types provides atomic storage and retrieval of an `Arc<T>`, in a sense similar to RCU: An
/// `Arc<T>`, which can be retrieved, providing a snapshot of the data, is kept. When it is
/// updated, the `Arc<T>` is replaced with the new data, and the old inner is first dropped when
/// all the current "snapshots" are dropped.
#[derive(Debug)]
pub struct ArcCell<T> {
    ptr: AtomicPtr<T>,
    sem: AtomicUsize,
}

impl<T> Drop for ArcCell<T> {
    fn drop(&mut self) {
        unsafe {
            Arc::from_raw(self.ptr.load(atomic::Ordering::Relaxed));
        }
    }
}

impl<T> ArcCell<T> {
    /// Creates a new `ArcCell`.
    pub fn new(arc: Arc<T>) -> ArcCell<T> {
        ArcCell {
            ptr: AtomicPtr::new(Arc::into_raw(arc)),
            sem: AtomicUsize::new(0),
        }
    }

    /// Stores a new value in the `ArcCell`, returning the previous
    /// value.
    pub fn set(&self, arc: Arc<T>) -> Arc<T> {
        let old = unsafe {
            Arc::from_raw(self.ptr.swap(Arc::into_raw(t), atomic::Ordering::Acquire))
        };

        while self.sem.load(atomic::Ordering::Relaxed) > 0 {}
        old
    }

    /// Returns a copy of the value stored by the `ArcCell`.
    pub fn get(&self) -> Arc<T> {
        self.sem.fetch_add(1, Ordering::Relaxed);
        let t = unsafe { Arc::from_raw(self.ptr.load(atomic::Ordering::SeqCst)) };
        // NB: correctness here depends on Arc's clone impl not panicking
        let out = t.clone();
        self.sem.fetch_sub(1, Ordering::Relaxed);
        mem::forget(t);
        out
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;
    use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn basic() {
        let r = ArcCell::new(Arc::new(0));
        assert_eq!(*r.get(), 0);
        assert_eq!(*r.set(Arc::new(1)), 0);
        assert_eq!(*r.get(), 1);
    }

    #[test]
    fn drop_runs() {
        static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;

        struct Foo;

        impl Drop for Foo {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        let r = ArcCell::new(Arc::new(Foo));
        let _f = r.get();
        r.get();
        r.set(Arc::new(Foo));
        drop(_f);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    }
}
