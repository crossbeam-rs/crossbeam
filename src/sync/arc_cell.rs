use std::{marker, mem};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A type providing atomic storage and retrieval of an `Arc<T>`.
#[derive(Debug)]
pub struct ArcCell<T> {
    ptr: AtomicUsize,
    sem: AtomicUsize,
    _marker: marker::PhantomData<Arc<T>>,
}

impl<T> Drop for ArcCell<T> {
    fn drop(&mut self) {
        unsafe {
            mem::transmute::<_, Arc<T>>(self.ptr.load(Ordering::Relaxed));
        }
    }
}

impl<T> ArcCell<T> {
    /// Creates a new `ArcCell`.
    pub fn new(t: Arc<T>) -> ArcCell<T> {
        ArcCell {
            ptr: AtomicUsize::new(unsafe { mem::transmute(t) }),
            sem: AtomicUsize::new(0),
            _marker: marker::PhantomData,
        }
    }

    /// Create a new `ArcCell` from the given `Arc` interior.
    pub fn with_val(v: T) -> ArcCell<T> {
        ArcCell {
            ptr: AtomicUsize::new(unsafe { mem::transmute(Arc::new(v)) }),
            sem: AtomicUsize::new(0),
            _marker: marker::PhantomData,
        }
    }

    /// Stores a new value in the `ArcCell`, returning the previous
    /// value.
    pub fn set(&self, t: Arc<T>) -> Arc<T> {
        unsafe {
            let t: usize = mem::transmute(t);
            let old: Arc<T> = mem::transmute(self.ptr.swap(t, Ordering::Acquire));
            while self.sem.load(Ordering::Relaxed) > 0 {}
            old
        }
    }

    /// Returns a copy of the value stored by the `ArcCell`.
    pub fn get(&self) -> Arc<T> {
        self.sem.fetch_add(1, Ordering::Relaxed);
        let t: Arc<T> = unsafe { mem::transmute(self.ptr.load(Ordering::SeqCst)) };
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
