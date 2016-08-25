use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A `MarkableArcCell` maintains an `Arc<T>` and a marker bit, providing atomic storage,
/// retrieval and CAS operations on the pair.
#[derive(Debug)]
pub struct MarkableArcCell<T> {
    ptr: AtomicUsize,
    sem: AtomicUsize,
    _marker: PhantomData<Arc<T>>,
}

impl<T> Drop for MarkableArcCell<T> {
    fn drop(&mut self) {
        unsafe { mem::transmute::<_, Arc<T>>(untag_val(self.ptr.load(Ordering::Relaxed)).0); }
    }
}

/// Tags an even usize with a boolean marker bit.
fn tag_val(p: usize, b: bool) -> usize {
    debug_assert!(p as usize & 1 == 0);
    p | b as usize
}

/// Retrieves the original value and the boolean marker bit from a tagged usize.
fn untag_val(t: usize) -> (usize, bool) {
    let mark = t & 1;
    (t - mark, mark == 1)
}

impl<T> MarkableArcCell<T> {
    /// Creates a new `MarkableArcCell` with the specified initial values of the `Arc` and the
    /// marker bit.
    pub fn new(t: Arc<T>, m: bool) -> MarkableArcCell<T> {
        MarkableArcCell {
            ptr: AtomicUsize::new(tag_val(unsafe { mem::transmute(t) }, m)),
            sem: AtomicUsize::new(0),
            _marker: PhantomData,
        }
    }

    /// Unconditionally sets the values of both the `Arc` and the marker bit and returns the
    /// previous ones.
    pub fn set(&self, t: Arc<T>, m: bool) -> (Arc<T>, bool) {
        unsafe {
            let t = tag_val(mem::transmute(t), m);
            let old = untag_val(self.ptr.swap(t, Ordering::Acquire));
            while self.sem.load(Ordering::Relaxed) > 0 {}
            (mem::transmute(old.0), old.1)
        }
    }

    /// Returns the current values of both the `Arc` and the marker bit.
    pub fn get(&self) -> (Arc<T>, bool) {
        self.sem.fetch_add(1, Ordering::Relaxed);
        let t = untag_val(self.ptr.load(Ordering::SeqCst));
        let t: (Arc<T>, bool) = unsafe { (mem::transmute(t.0), t.1) };
        let out = (t.0.clone(), t.1);
        self.sem.fetch_sub(1, Ordering::Relaxed);
        mem::forget(t);
        out
    }

    /// Returns the current value of the `Arc`.
    pub fn get_arc(&self) -> Arc<T> {
        self.sem.fetch_add(1, Ordering::Relaxed);
        let t = untag_val(self.ptr.load(Ordering::SeqCst)).0;
        let t: Arc<T> = unsafe { mem::transmute(t) };
        let out = t.clone();
        self.sem.fetch_sub(1, Ordering::Relaxed);
        mem::forget(t);
        out
    }

    /// Returns the current value of the marker bit.
    pub fn is_marked(&self) -> bool {
        self.ptr.load(Ordering::Relaxed) & 1 == 1
    }

    /// Atomically sets the value of the marker bit if the current value of the `Arc` is the same
    /// as the specified `current` value. Returns `true` iff the new value was written.
    ///
    /// Two `Arc`s are said to be the same iff they manage the same object. If we were to imagine
    /// a function `fn is_same<T>(a: Arc<T>, b: Arc<T>) -> bool`, the following would hold:
    ///
    /// `is_same(Arc::new(2), Arc::new(2)) == false`
    ///
    /// `let v = Arc::new(2); is_same(v.clone(), v) == true`
    pub fn compare_arc_exchange_mark(&self, current_t: Arc<T>, new_m: bool) -> bool {
        let current: usize = unsafe { mem::transmute(current_t) };
        drop::<Arc<T>>(unsafe { mem::transmute(current) });

        let mut t = untag_val(self.ptr.load(Ordering::Relaxed));
        while t.0 == current {
            if t.1 == new_m {
                return true;
            }

            if let Ok(_) = self.ptr.compare_exchange_weak(
                tag_val(t.0, t.1),
                tag_val(t.0, new_m),
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                return true;
            }

            t = untag_val(self.ptr.load(Ordering::Relaxed));
        }

        return false;
    }

    /// Atomically sets the values of both the `Arc` and the marker bit if the current values of
    /// both are the same as the specified `current` values. Returns `true` iff the new values were
    /// written.
    ///
    /// Two `Arc`s are said to be the same iff they manage the same object. If we were to imagine
    /// a function `fn is_same<T>(a: Arc<T>, b: Arc<T>) -> bool`, the following would hold:
    ///
    /// `is_same(Arc::new(2), Arc::new(2)) == false`
    ///
    /// `let v = Arc::new(2); is_same(v.clone(), v) == true`
    pub fn compare_exchange(&self, current_t: Arc<T>, new_t: Arc<T>, current_m: bool, new_m: bool) -> bool {
        let current: usize = unsafe { mem::transmute(current_t) };
        drop::<Arc<T>>(unsafe { mem::transmute(current) });
        let current = tag_val(current, current_m);

        let new: usize = unsafe { mem::transmute(new_t) };
        let new = tag_val(new, new_m);

        match self.ptr.compare_exchange(current, new, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(prev) => {
                while self.sem.load(Ordering::Relaxed) > 0 {}
                drop::<Arc<T>>(unsafe { mem::transmute(untag_val(prev).0) } );
                return true;
            },
            Err(_) => {
                drop::<Arc<T>>(unsafe { mem::transmute(untag_val(new).0) } );
                return false;
            },
        }
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;
    use std::sync::atomic::{ATOMIC_USIZE_INIT, AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn basic() {
        let r = MarkableArcCell::new(Arc::new(3), false);

        let vals = r.get();
        assert_eq!(*vals.0, 3);
        assert_eq!(vals.1, false);

        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.is_marked(), false);

        let prev = r.set(Arc::new(2), true);
        assert_eq!(*prev.0, 3);
        assert_eq!(prev.1, false);

        assert_eq!(*r.get_arc(), 2);
        assert_eq!(r.is_marked(), true);
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

        let r = MarkableArcCell::new(Arc::new(Foo), false);
        let _f = r.get().0;
        r.get_arc();
        r.set(Arc::new(Foo), false);
        drop(_f);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cmpxchg_works() {
        let r = MarkableArcCell::new(Arc::new(1), true);

        let st = r.compare_arc_exchange_mark(r.get_arc(), false);
        assert_eq!(st, true);
        assert_eq!(*r.get_arc(), 1);
        assert_eq!(r.is_marked(), false);

        let st = r.compare_exchange(r.get_arc(), Arc::new(3), false, true);
        assert_eq!(st, true);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.is_marked(), true);

        let st = r.compare_arc_exchange_mark(Arc::new(3), false);
        assert_eq!(st, false);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.is_marked(), true);

        let st = r.compare_exchange(r.get_arc(), Arc::new(4), false, false);
        assert_eq!(st, false);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.is_marked(), true);

        let st = r.compare_exchange(Arc::new(0), Arc::new(4), true, false);
        assert_eq!(st, false);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.is_marked(), true);
    }

    #[test]
    fn concurrency_works() {



    }
}
