/// Epoch-based garbage collector.
///
/// # Examples
///
/// ```
/// use crossbeam_epoch::Collector;
///
/// let collector = Collector::new();
///
/// let handle = collector.register();
/// drop(collector); // `handle` still works after dropping `collector`
///
/// handle.pin().flush();
/// ```
use core::fmt;

use crate::{
    guard::Guard,
    internal::{Global, Local},
    primitive::sync::Arc,
};

/// An epoch-based garbage collector.
pub struct Collector {
    pub(crate) global: Arc<Global>,
}

unsafe impl Send for Collector {}
unsafe impl Sync for Collector {}

impl Default for Collector {
    #[allow(clippy::arc_with_non_send_sync)] // https://github.com/rust-lang/rust-clippy/issues/11382
    fn default() -> Self {
        Self {
            global: Arc::new(Global::new()),
        }
    }
}

impl Collector {
    /// Creates a new collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new handle for the collector.
    pub fn register(&self) -> LocalHandle {
        Local::register(self)
    }
}

impl Clone for Collector {
    /// Creates another reference to the same garbage collector.
    fn clone(&self) -> Self {
        Self {
            global: self.global.clone(),
        }
    }
}

impl fmt::Debug for Collector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Collector { .. }")
    }
}

impl PartialEq for Collector {
    /// Checks if both handles point to the same collector.
    fn eq(&self, rhs: &Self) -> bool {
        Arc::ptr_eq(&self.global, &rhs.global)
    }
}
impl Eq for Collector {}

/// A handle to a garbage collector.
pub struct LocalHandle {
    pub(crate) local: *const Local,
}

impl LocalHandle {
    /// Pins the handle.
    #[inline]
    pub fn pin(&self) -> Guard {
        unsafe { (*self.local).pin() }
    }

    /// Returns `true` if the handle is pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        unsafe { (*self.local).is_pinned() }
    }

    /// Returns the `Collector` associated with this handle.
    #[inline]
    pub fn collector(&self) -> &Collector {
        unsafe { (*self.local).collector() }
    }
}

impl Drop for LocalHandle {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            Local::release_handle(self.local);
        }
    }
}

impl fmt::Debug for LocalHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("LocalHandle { .. }")
    }
}

#[cfg(all(test, not(crossbeam_loom)))]
#[allow(
    clippy::alloc_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core
)]
mod tests {
    use std::{
        mem::ManuallyDrop,
        sync::atomic::{AtomicUsize, Ordering},
        vec::Vec,
    };

    use crossbeam_utils::thread;

    use crate::{Collector, Owned};

    const NUM_THREADS: usize = 8;

    #[test]
    fn pin_reentrant() {
        let collector = Collector::new();
        let handle = collector.register();
        drop(collector);

        assert!(!handle.is_pinned());
        {
            let _guard = &handle.pin();
            assert!(handle.is_pinned());
            {
                let _guard = &handle.pin();
                assert!(handle.is_pinned());
            }
            assert!(handle.is_pinned());
        }
        assert!(!handle.is_pinned());
    }

    #[test]
    fn flush_local_bag() {
        let collector = Collector::new();
        let handle = collector.register();
        drop(collector);

        for _ in 0..100 {
            let guard = &handle.pin();
            unsafe {
                let a = Owned::new(7).into_shared(guard);
                guard.defer_destroy(a);

                assert!(!(*guard.local).bag.with(|b| (*b).is_empty()));

                while !(*guard.local).bag.with(|b| (*b).is_empty()) {
                    guard.flush();
                }
            }
        }
    }

    #[test]
    fn garbage_buffering() {
        let collector = Collector::new();
        let handle = collector.register();
        drop(collector);

        let guard = &handle.pin();
        unsafe {
            for _ in 0..10 {
                let a = Owned::new(7).into_shared(guard);
                guard.defer_destroy(a);
            }
            assert!(!(*guard.local).bag.with(|b| (*b).is_empty()));
        }
    }

    #[test]
    fn pin_holds_advance() {
        const N: usize = if cfg!(miri) { 500 } else { 500_000 };

        let collector = Collector::new();

        thread::scope(|scope| {
            for _ in 0..NUM_THREADS {
                scope.spawn(|_| {
                    let handle = collector.register();
                    for _ in 0..N {
                        let guard = &handle.pin();

                        let before = collector.global.epoch.load(Ordering::Relaxed);
                        collector.global.collect(guard);
                        let after = collector.global.epoch.load(Ordering::Relaxed);

                        assert!(after.wrapping_sub(before) <= 2);
                    }
                });
            }
        })
        .unwrap();
    }

    #[test]
    // TODO: assertions failed due to `cfg(crossbeam_sanitize)` reduce `internal::MAX_OBJECTS`: https://github.com/crossbeam-rs/crossbeam/issues/662
    #[cfg_attr(crossbeam_sanitize, ignore)]
    fn incremental() {
        const COUNT: usize = if cfg!(miri) { 500 } else { 100_000 };
        static DESTROYS: AtomicUsize = AtomicUsize::new(0);

        let collector = Collector::new();
        let handle = collector.register();

        unsafe {
            let guard = &handle.pin();
            for _ in 0..COUNT {
                let a = Owned::new(7i32).into_shared(guard);
                guard.defer_unchecked(move || {
                    drop(a.into_owned());
                    DESTROYS.fetch_add(1, Ordering::Relaxed);
                });
            }
            guard.flush();
        }

        let mut last = 0;

        while last < COUNT {
            let curr = DESTROYS.load(Ordering::Relaxed);
            assert!(curr - last <= 1024);
            last = curr;

            let guard = &handle.pin();
            collector.global.collect(guard);
        }
        assert!(DESTROYS.load(Ordering::Relaxed) == COUNT);
    }

    #[test]
    fn buffering() {
        const COUNT: usize = 10;
        const N: usize = if cfg!(miri) { 500 } else { 100_000 };
        static DESTROYS: AtomicUsize = AtomicUsize::new(0);

        let collector = Collector::new();
        let handle = collector.register();

        unsafe {
            let guard = &handle.pin();
            for _ in 0..COUNT {
                let a = Owned::new(7i32).into_shared(guard);
                guard.defer_unchecked(move || {
                    drop(a.into_owned());
                    DESTROYS.fetch_add(1, Ordering::Relaxed);
                });
            }
        }

        for _ in 0..N {
            collector.global.collect(&handle.pin());
        }
        assert!(DESTROYS.load(Ordering::Relaxed) < COUNT);

        handle.pin().flush();

        while DESTROYS.load(Ordering::Relaxed) < COUNT {
            let guard = &handle.pin();
            collector.global.collect(guard);
        }
        assert_eq!(DESTROYS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn count_drops() {
        const COUNT: usize = if cfg!(miri) { 500 } else { 100_000 };
        static DROPS: AtomicUsize = AtomicUsize::new(0);

        struct Elem(#[allow(dead_code)] i32);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        let collector = Collector::new();
        let handle = collector.register();

        unsafe {
            let guard = &handle.pin();

            for _ in 0..COUNT {
                let a = Owned::new(Elem(7i32)).into_shared(guard);
                guard.defer_destroy(a);
            }
            guard.flush();
        }

        while DROPS.load(Ordering::Relaxed) < COUNT {
            let guard = &handle.pin();
            collector.global.collect(guard);
        }
        assert_eq!(DROPS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn count_destroy() {
        const COUNT: usize = if cfg!(miri) { 500 } else { 100_000 };
        static DESTROYS: AtomicUsize = AtomicUsize::new(0);

        let collector = Collector::new();
        let handle = collector.register();

        unsafe {
            let guard = &handle.pin();

            for _ in 0..COUNT {
                let a = Owned::new(7i32).into_shared(guard);
                guard.defer_unchecked(move || {
                    drop(a.into_owned());
                    DESTROYS.fetch_add(1, Ordering::Relaxed);
                });
            }
            guard.flush();
        }

        while DESTROYS.load(Ordering::Relaxed) < COUNT {
            let guard = &handle.pin();
            collector.global.collect(guard);
        }
        assert_eq!(DESTROYS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn drop_array() {
        const COUNT: usize = 700;
        static DROPS: AtomicUsize = AtomicUsize::new(0);

        struct Elem(#[allow(dead_code)] i32);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        let collector = Collector::new();
        let handle = collector.register();

        let mut guard = handle.pin();

        let mut v = Vec::with_capacity(COUNT);
        for i in 0..COUNT {
            v.push(Elem(i as i32));
        }

        {
            let a = Owned::new(v).into_shared(&guard);
            unsafe {
                guard.defer_destroy(a);
            }
            guard.flush();
        }

        while DROPS.load(Ordering::Relaxed) < COUNT {
            guard.repin();
            collector.global.collect(&guard);
        }
        assert_eq!(DROPS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn destroy_array() {
        const COUNT: usize = if cfg!(miri) { 500 } else { 100_000 };
        static DESTROYS: AtomicUsize = AtomicUsize::new(0);

        let collector = Collector::new();
        let handle = collector.register();

        unsafe {
            let guard = &handle.pin();

            let mut v = Vec::with_capacity(COUNT);
            for i in 0..COUNT {
                v.push(i as i32);
            }

            let len = v.len();
            let cap = v.capacity();
            let ptr = ManuallyDrop::new(v).as_mut_ptr();
            guard.defer_unchecked(move || {
                drop(Vec::from_raw_parts(ptr, len, cap));
                DESTROYS.fetch_add(len, Ordering::Relaxed);
            });
            guard.flush();
        }

        while DESTROYS.load(Ordering::Relaxed) < COUNT {
            let guard = &handle.pin();
            collector.global.collect(guard);
        }
        assert_eq!(DESTROYS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn stress() {
        const THREADS: usize = 8;
        const COUNT: usize = if cfg!(miri) { 500 } else { 100_000 };
        static DROPS: AtomicUsize = AtomicUsize::new(0);

        struct Elem(#[allow(dead_code)] i32);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        let collector = Collector::new();

        thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|_| {
                    let handle = collector.register();
                    for _ in 0..COUNT {
                        let guard = &handle.pin();
                        unsafe {
                            let a = Owned::new(Elem(7i32)).into_shared(guard);
                            guard.defer_destroy(a);
                        }
                    }
                });
            }
        })
        .unwrap();

        let handle = collector.register();
        while DROPS.load(Ordering::Relaxed) < COUNT * THREADS {
            let guard = &handle.pin();
            collector.global.collect(guard);
        }
        assert_eq!(DROPS.load(Ordering::Relaxed), COUNT * THREADS);
    }

    // A participant that exits is unlinked from the list of participants by another thread, and
    // the thread that later advances the epoch may never see its entry. The accesses of the
    // exited participant must still happen before the reclamation of what it accessed.
    #[test]
    fn exited_participant_accesses_happen_before_reclamation() {
        use std::sync::{
            Barrier,
            atomic::Ordering::{AcqRel, Acquire, Relaxed, Release},
        };

        use crate::{Atomic, Shared};

        struct Elem(u64);

        static DROPS: AtomicUsize = AtomicUsize::new(0);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Relaxed);
            }
        }

        // The steps are ordered with `Relaxed` accesses only, so that the test does not add any
        // synchronization of its own.
        let step = AtomicUsize::new(0);
        let wait = |n: usize| {
            while step.load(Relaxed) != n {
                std::thread::yield_now();
            }
        };

        let collector = Collector::new();
        let shared = Atomic::new(Elem(7));
        let registered = AtomicUsize::new(0);
        let barrier = Barrier::new(5);

        // Registered in this order, the list of participants is [b, x, p, t, q].
        let roles = ["q", "t", "p", "x", "b"];

        thread::scope(|scope| {
            for (i, &role) in roles.iter().enumerate() {
                let (collector, shared, registered, barrier, step, wait) =
                    (&collector, &shared, &registered, &barrier, &step, &wait);
                scope.spawn(move |_| {
                    while registered.load(Acquire) != i {
                        std::thread::yield_now();
                    }
                    let handle = collector.register();
                    registered.store(i + 1, Release);
                    // The first pin of a handle collects; later pins in this test do not.
                    drop(handle.pin());
                    barrier.wait();

                    match role {
                        "q" => {
                            // Pins in the current epoch E and holds it there.
                            wait(0);
                            let guard = handle.pin();
                            step.store(1, Relaxed);
                            wait(5);
                            drop(guard);
                            step.store(6, Relaxed);
                        }
                        "t" => {
                            // Reads the element, then exits.
                            wait(1);
                            let guard = handle.pin();
                            let elem = shared.load(Acquire, &guard);
                            assert_eq!(unsafe { elem.deref() }.0, 7);
                            step.store(2, Relaxed);
                            wait(3);
                            drop(guard);
                            drop(handle);
                            step.store(4, Relaxed);
                            return;
                        }
                        "p" => {
                            // Removes the element, defers its destruction and advances to E + 1.
                            wait(2);
                            let guard = handle.pin();
                            let elem = shared.swap(Shared::null(), AcqRel, &guard);
                            unsafe { guard.defer_destroy(elem) };
                            guard.flush();
                            drop(guard);
                            step.store(3, Relaxed);
                        }
                        "x" => {
                            // Unlinks the entry of `t`, then cannot advance because of `q`. Stays
                            // pinned: if it unpinned, `b` would synchronize with it.
                            wait(4);
                            let guard = handle.pin();
                            guard.flush();
                            step.store(5, Relaxed);
                            wait(7);
                            drop(guard);
                        }
                        "b" => {
                            // Never sees the entry of `t`, advances to E + 2 and destroys the
                            // element. The loop is bounded because `x` may have pinned in a stale
                            // epoch, which holds the epoch back until `x` unpins.
                            wait(6);
                            for _ in 0..100 {
                                if DROPS.load(Relaxed) != 0 {
                                    break;
                                }
                                handle.pin().flush();
                            }
                            step.store(7, Relaxed);
                        }
                        _ => unreachable!(),
                    }
                    wait(7);
                });
            }
        })
        .unwrap();

        drop(collector);
        assert_eq!(DROPS.load(Relaxed), 1);
    }

    #[test]
    fn use_guard_after_handle_and_collector_dropped() {
        let collector = Collector::new();
        let handle = collector.register();

        let guard = handle.pin();

        drop(collector);
        drop(handle);

        guard.flush();
    }
}
