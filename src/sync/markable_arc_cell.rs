use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};

/// A `MarkableArcCell` maintains an `Arc<T>` and a marker bit, providing atomic storage and
/// retrieval of both, as well as atomic `CAS` operations on them.
#[derive(Debug)]
pub struct MarkableArcCell<T>(AtomicUsize, AtomicBool, PhantomData<Arc<T>>);

impl<T> Drop for MarkableArcCell<T> {
    fn drop(&mut self) {
        self.take();
    }
}

impl<T> MarkableArcCell<T> {
    /// Creates a new `MarkableArcCell` with the specified initial values of the `Arc` and the
    /// marker bit.
    pub fn new(t: Arc<T>, m: bool) -> MarkableArcCell<T> {
        MarkableArcCell(AtomicUsize::new(unsafe { mem::transmute(t) }), AtomicBool::new(m), PhantomData)
    }

    /// Creates a new `MarkableArcCell` with the specified initial values of the `Arc` interior
    /// and the marker bit.
    pub fn with_val(v: T, m: bool) -> MarkableArcCell<T> {
        MarkableArcCell(AtomicUsize::new(unsafe { mem::transmute(Arc::new(v)) }), AtomicBool::new(m), PhantomData)
    }

    // Locks the internal spinlock.
    fn take(&self) -> Arc<T> {
        loop {
            match self.0.swap(0, Ordering::Acquire) {
                0 => {}
                n => return unsafe { mem::transmute(n) }
            }
        }
    }

    // Unlocks the internal spinlock.
    fn put(&self, t: Arc<T>) {
        debug_assert_eq!(self.0.load(Ordering::SeqCst), 0);
        self.0.store(unsafe { mem::transmute(t) }, Ordering::Release);
    }

    /// Unconditionally sets the `Arc` value and returns the previous one.
    pub fn set_arc(&self, t: Arc<T>) -> Arc<T> {
        let old = self.take();
        self.put(t);
        old
    }

    /// Returns a copy of the current `Arc` value.
    pub fn get_arc(&self) -> Arc<T> {
        let t = self.take();
        // NB: correctness here depends on Arc's clone impl not panicking
        let out = t.clone();
        self.put(t);
        out
    }

    /// Unconditionally sets the marker bit value and returns the previous one.
    pub fn set_mark(&self, m: bool) -> bool {
        self.1.swap(m, Ordering::AcqRel)
    }

    /// Returns the current marker bit value.
    pub fn get_mark(&self) -> bool {
        self.1.load(Ordering::Acquire)
    }

    /// Unconditionally sets the values of both the `Arc` and the marker bit and returns the
    /// previous ones.
    pub fn set(&self, t: Arc<T>, m: bool) -> (Arc<T>, bool) {
        let old = self.take();
        let old_both = (old, self.1.swap(m, Ordering::AcqRel));
        self.put(t);
        old_both
    }

    /// Returns the current values of both the `Arc` and the marker bit.
    pub fn get(&self) -> (Arc<T>, bool) {
        let t = self.take();
        let out = (t.clone(), self.1.load(Ordering::Acquire));
        self.put(t);
        out
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
        let current_t: usize = unsafe { mem::transmute(current_t) };
        drop::<Arc<T>>(unsafe { mem::transmute(current_t) });

        let t: usize = unsafe { mem::transmute(self.take()) };
        if t == current_t {
            self.1.store(new_m, Ordering::Release);
            self.put(unsafe { mem::transmute(t) });
            return true;
        }
        else {
            self.put(unsafe { mem::transmute(t) });
            return false;
        }
    }

    /// Atomically sets the value of the `Arc` if the current value of the marker bit is the same
    /// as the specified `current` value. Returns `true` iff the new value was written.
    pub fn compare_mark_exchange_arc(&self, current_m: bool, new_t: Arc<T>) -> bool {
        let t = self.take();
        if self.1.load(Ordering::Acquire) == current_m {
            self.put(new_t);
            return true;
        }
        else {
            self.put(t);
            return false;
        }
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
        // get a usize representation of current and then drop it
        let current_t: usize = unsafe { mem::transmute(current_t) };
        drop::<Arc<T>>(unsafe { mem::transmute(current_t) });

        // this locks the value, meaning it will necessarily stay alive
        let t: usize = unsafe { mem::transmute(self.take()) };
        if t == current_t && self.1.load(Ordering::Acquire) == current_m {
            self.1.store(new_m, Ordering::Release);
            self.put(new_t);
            drop::<Arc<T>>( unsafe { mem::transmute(t) } );
            return true;
        }
        else {
            self.put(unsafe { mem::transmute(t) });
            return false;
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
        let r = MarkableArcCell::with_val(0, false);

        assert_eq!(*r.get_arc(), 0);
        assert_eq!(r.get_mark(), false);
        let vals = r.get();
        assert_eq!(*vals.0, 0);
        assert_eq!(vals.1, false);

        assert_eq!(*r.set_arc(Arc::new(1)), 0);
        assert_eq!(*r.get_arc(), 1);

        assert_eq!(r.set_mark(true), false);
        assert_eq!(r.get_mark(), true);

        let old = r.set(Arc::new(2), false);
        assert_eq!(*old.0, 1);
        assert_eq!(old.1, true);

        let now = r.get();
        assert_eq!(*now.0, 2);
        assert_eq!(now.1, false);
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

        let r = MarkableArcCell::with_val(Foo, false);
        let _f = r.get_arc();
        r.get_arc();
        r.set_arc(Arc::new(Foo));
        drop(_f);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
        drop(r);
        assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cmpxchg_works() {
        let r = MarkableArcCell::with_val(1, true);

        r.compare_arc_exchange_mark(r.get_arc(), false);
        assert_eq!(*r.get_arc(), 1);
        assert_eq!(r.get_mark(), false);

        r.compare_mark_exchange_arc(false, Arc::new(2));
        assert_eq!(*r.get_arc(), 2);
        assert_eq!(r.get_mark(), false);

        r.compare_exchange(r.get_arc(), Arc::new(3), false, true);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.get_mark(), true);

        r.compare_arc_exchange_mark(Arc::new(0), false);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.get_mark(), true);

        r.compare_mark_exchange_arc(false, Arc::new(0));
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.get_mark(), true);

        r.compare_exchange(r.get_arc(), Arc::new(4), false, false);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.get_mark(), true);

        r.compare_exchange(Arc::new(0), Arc::new(4), true, false);
        assert_eq!(*r.get_arc(), 3);
        assert_eq!(r.get_mark(), true);
    }
}
