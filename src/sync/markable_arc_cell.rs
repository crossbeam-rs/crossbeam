use std::marker::PhantomData;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};

/// This type contains two values: and `Arc<T>` and a marker bit. Conditional atomic operations can
/// be performed on the pair, or the values can be managed separately, also atomically.
#[derive(Debug)]
pub struct MarkableArcCell<T>(AtomicUsize, AtomicBool, PhantomData<Arc<T>>);

impl<T> Drop for MarkableArcCell<T> {
    fn drop(&mut self) {
        self.take();
    }
}

impl<T> MarkableArcCell<T> {
    /// Creates a new `MarkableArcCell` with given initial values of the `Arc<T>` and the marker bit.
    pub fn new(t: Arc<T>, m: bool) -> MarkableArcCell<T> {
        MarkableArcCell(AtomicUsize::new(unsafe { mem::transmute(t) }), AtomicBool::new(m), PhantomData)
    }

    /// Creates a new `MarkableArcCell` with given initial values of the `Arc<T>` interior and the marker bit.
    pub fn with_val(t: T, m: bool) -> MarkableArcCell<T> {
        MarkableArcCell(AtomicUsize::new(unsafe { mem::transmute(Arc::new(t)) }), AtomicBool::new(m), PhantomData)
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

    /// Uncoditionally sets the `Arc<T>` value and returns the previous one.
    pub fn set_arc(&self, t: Arc<T>) -> Arc<T> {
        let old = self.take();
        self.put(t);
        old
    }

    /// Returns a copy of the `Arc<T>` value.
    pub fn get_arc(&self) -> Arc<T> {
        let t = self.take();
        // NB: correctness here depends on Arc's clone impl not panicking
        let out = t.clone();
        self.put(t);
        out
    }

    /// Unconditionally sets the marker bit and returns its previous value.
    pub fn set_mark(&self, m: bool) -> bool {
        self.1.swap(m, Ordering::AcqRel)
    }

    /// Returns the marker bit value.
    pub fn get_mark(&self) -> bool {
        self.1.load(Ordering::Acquire)
    }


    /// Unconditionally sets both values to the given ones and returns the previous values.
    pub fn set(&self, t: Arc<T>, m: bool) -> (Arc<T>, bool) {
        let old = self.take();
        let old_both = (old, self.1.swap(m, Ordering::AcqRel));
        self.put(t);
        old_both
    }

    /// Returns the contained values of `Arc<T>` and marker bit.
    pub fn get(&self) -> (Arc<T>, bool) {
        let t = self.take();
        let out = (t.clone(), self.1.load(Ordering::Acquire));
        self.put(t);
        out
    }

    // +-------------------------------+
    // |TODO return old values in CASs?|
    // +-------------------------------+
    // TODO make better interface -> parameter and fn names should match stuff in std::sync::atomic
    //
    // TODO DOCS DOCS DOCS HOW BAD CAN THEY BE REALLY WTH

    /// Atomically sets the marker bit to the given value if the `Arc<T>` in this
    /// `MarkableArcCell<T>' manages the same object as the provided `Arc<T>`.
    /// Returns a value indicating whether the operation was successful.
    pub fn compare_arc_exchange_mark(&self, current: Arc<T>, m: bool) -> bool {
        let current: usize = unsafe { mem::transmute(current) };
        drop::<Arc<T>>(unsafe { mem::transmute(current) });

        let t: usize = unsafe { mem::transmute(self.take()) };
        if t == current {
            self.1.store(m, Ordering::Release);
            self.put(unsafe { mem::transmute(t) });
            return true;
        }
        else {
            self.put(unsafe { mem::transmute(t) });
            return false;
        }
    }

    /// Atomically sets the `Arc<T>` value to the given value if the marker bit is set to the same
    /// value as the provided one. Returns a value indicating whether the operation was
    /// successful.
    pub fn compare_mark_exchange_arc(&self, expected: bool, t: Arc<T>) -> bool {
        let curr = self.take();
        if self.1.load(Ordering::Acquire) == expected {
            self.put(t);
            return true;
        }
        self.put(curr);
        return false;
    }

    /// Stores new values in the `MarkableArcCell` to the provided `new` values if the `Arc<T>`
    /// value manages the same object as the provided `curr` `Arc<T>` and the marker bit is set to
    /// the same value as the provided `curr` value. Returns a value indicating whether the
    /// operation was successful.
    pub fn compare_exchange(&self, currv: Arc<T>, newv: Arc<T>, currm: bool, newm: bool) -> bool {
        // get a usize representation of current and then drop it
        let currv: usize = unsafe { mem::transmute(currv) };
        drop::<Arc<T>>(unsafe { mem::transmute(currv) });

        // this locks the value, meaning it will necessarily stay alive
        let t: usize = unsafe { mem::transmute(self.take()) };
        if t == currv && self.1.load(Ordering::Acquire) == currm {
            self.1.store(newm, Ordering::Release);
            self.put(newv);
            drop::<Arc<T>>( unsafe { mem::transmute(t) } );
            return true;
        }
        self.put(unsafe { mem::transmute(t) });
        return false;
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
