/// Epoch-based garbage collector.
///
/// # Examples
///
/// ```
/// use crossbeam_epoch::Collector;
///
/// let collector = Collector::new();
///
/// let handle = collector.handle();
/// drop(collector); // `handle` still works after dropping `collector`
///
/// handle.pin(|scope| {
///     scope.flush();
/// });
/// ```

use std::sync::Arc;
use internal::{Global, Local};
use scope::Scope;

/// An epoch-based garbage collector.
pub struct Collector(Arc<Global>);

/// A handle to a garbage collector.
pub struct Handle {
    global: Arc<Global>,
    local: Local,
}

impl Collector {
    /// Creates a new collector.
    pub fn new() -> Self {
        Self { 0: Arc::new(Global::new()) }
    }

    /// Creates a new handle for the collector.
    #[inline]
    pub fn handle(&self) -> Handle {
        Handle::new(self.0.clone())
    }
}

impl Handle {
    fn new(global: Arc<Global>) -> Self {
        let local = Local::new(&global);
        Self { global, local }
    }

    /// Pin the current handle.
    #[inline]
    pub fn pin<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Scope) -> R,
    {
        unsafe { self.local.pin(&self.global, f) }
    }

    /// Check if the current handle is pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.local.is_pinned()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { self.local.unregister(&self.global) }
    }
}


#[cfg(test)]
mod tests {
    use std::mem;
    use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT};
    use std::sync::atomic::Ordering;
    use crossbeam_utils::scoped;

    use {Collector, Owned};

    const NUM_THREADS: usize = 8;

    #[test]
    fn pin_reentrant() {
        let collector = Collector::new();
        let handle = collector.handle();
        drop(collector);

        assert!(!handle.is_pinned());
        handle.pin(|_| {
            assert!(handle.is_pinned());
            handle.pin(|_| {
                assert!(handle.is_pinned());
            });
            assert!(handle.is_pinned());
        });
        assert!(!handle.is_pinned());
    }

    #[test]
    fn flush_local_bag() {
        let collector = Collector::new();
        let handle = collector.handle();
        drop(collector);

        for _ in 0..100 {
            handle.pin(|scope| unsafe {
                let a = Owned::new(7).into_ptr(scope);
                scope.defer(move || a.into_owned());

                assert!(!(*scope.bag).is_empty());

                while !(*scope.bag).is_empty() {
                    scope.flush();
                }
            });
        }
    }

    #[test]
    fn garbage_buffering() {
        let collector = Collector::new();
        let handle = collector.handle();
        drop(collector);

        handle.pin(|scope| unsafe {
            for _ in 0..10 {
                let a = Owned::new(7).into_ptr(scope);
                scope.defer(move || a.into_owned());
            }
            assert!(!(*scope.bag).is_empty());
        });
    }

    #[test]
    fn pin_holds_advance() {
        let collector = Collector::new();

        let threads = (0..NUM_THREADS)
            .map(|_| {
                scoped::scope(|scope| {
                    scope.spawn(|| {
                        let handle = collector.handle();
                        for _ in 0..500_000 {
                            handle.pin(|scope| {
                                let before = collector.0.get_epoch();
                                collector.0.collect(scope);
                                let after = collector.0.get_epoch();

                                assert!(after.wrapping_sub(before) <= 2);
                            });
                        }
                    })
                })
            })
            .collect::<Vec<_>>();
        drop(collector);

        for t in threads {
            t.join();
        }
    }

    #[test]
    fn incremental() {
        const COUNT: usize = 100_000;
        static DESTROYS: AtomicUsize = ATOMIC_USIZE_INIT;

        let collector = Collector::new();
        let handle = collector.handle();

        handle.pin(|scope| unsafe {
            for _ in 0..COUNT {
                let a = Owned::new(7i32).into_ptr(scope);
                scope.defer(move || {
                    drop(a.into_owned());
                    DESTROYS.fetch_add(1, Ordering::Relaxed);
                });
            }
            scope.flush();
        });

        let mut last = 0;

        while last < COUNT {
            let curr = DESTROYS.load(Ordering::Relaxed);
            assert!(curr - last <= 1024);
            last = curr;

            handle.pin(|scope| collector.0.collect(scope));
        }
        assert!(DESTROYS.load(Ordering::Relaxed) == 100_000);
    }

    #[test]
    fn buffering() {
        const COUNT: usize = 10;
        static DESTROYS: AtomicUsize = ATOMIC_USIZE_INIT;

        let collector = Collector::new();
        let handle = collector.handle();

        handle.pin(|scope| unsafe {
            for _ in 0..COUNT {
                let a = Owned::new(7i32).into_ptr(scope);
                scope.defer(move || {
                    drop(a.into_owned());
                    DESTROYS.fetch_add(1, Ordering::Relaxed);
                });
            }
        });

        for _ in 0..100_000 {
            handle.pin(|scope| { collector.0.collect(scope); });
        }
        assert!(DESTROYS.load(Ordering::Relaxed) < COUNT);

        handle.pin(|scope| scope.flush());

        while DESTROYS.load(Ordering::Relaxed) < COUNT {
            handle.pin(|scope| collector.0.collect(scope));
        }
        assert_eq!(DESTROYS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn count_drops() {
        const COUNT: usize = 100_000;
        static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;

        struct Elem(i32);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        let collector = Collector::new();
        let handle = collector.handle();

        handle.pin(|scope| unsafe {
            for _ in 0..COUNT {
                let a = Owned::new(Elem(7i32)).into_ptr(scope);
                scope.defer(move || a.into_owned());
            }
            scope.flush();
        });

        while DROPS.load(Ordering::Relaxed) < COUNT {
            handle.pin(|scope| collector.0.collect(scope));
        }
        assert_eq!(DROPS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn count_destroy() {
        const COUNT: usize = 100_000;
        static DESTROYS: AtomicUsize = ATOMIC_USIZE_INIT;

        let collector = Collector::new();
        let handle = collector.handle();

        handle.pin(|scope| unsafe {
            for _ in 0..COUNT {
                let a = Owned::new(7i32).into_ptr(scope);
                scope.defer(move || {
                    drop(a.into_owned());
                    DESTROYS.fetch_add(1, Ordering::Relaxed);
                });
            }
            scope.flush();
        });

        while DESTROYS.load(Ordering::Relaxed) < COUNT {
            handle.pin(|scope| collector.0.collect(scope));
        }
        assert_eq!(DESTROYS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn drop_array() {
        const COUNT: usize = 700;
        static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;

        struct Elem(i32);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        let collector = Collector::new();
        let handle = collector.handle();

        handle.pin(|scope| unsafe {
            let mut v = Vec::with_capacity(COUNT);
            for i in 0..COUNT {
                v.push(Elem(i as i32));
            }

            let a = Owned::new(v).into_ptr(scope);
            scope.defer(move || a.into_owned());
            scope.flush();
        });

        while DROPS.load(Ordering::Relaxed) < COUNT {
            handle.pin(|scope| collector.0.collect(scope));
        }
        assert_eq!(DROPS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn destroy_array() {
        const COUNT: usize = 100_000;
        static DESTROYS: AtomicUsize = ATOMIC_USIZE_INIT;

        let collector = Collector::new();
        let handle = collector.handle();

        handle.pin(|scope| unsafe {
            let mut v = Vec::with_capacity(COUNT);
            for i in 0..COUNT {
                v.push(i as i32);
            }

            let ptr = v.as_mut_ptr() as usize;
            let len = v.len();
            scope.defer(move || {
                drop(Vec::from_raw_parts(ptr as *const u8 as *mut u8, len, len));
                DESTROYS.fetch_add(len, Ordering::Relaxed);
            });
            scope.flush();

            mem::forget(v);
        });

        while DESTROYS.load(Ordering::Relaxed) < COUNT {
            handle.pin(|scope| collector.0.collect(scope));
        }
        assert_eq!(DESTROYS.load(Ordering::Relaxed), COUNT);
    }

    #[test]
    fn stress() {
        const THREADS: usize = 8;
        const COUNT: usize = 100_000;
        static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;

        struct Elem(i32);

        impl Drop for Elem {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        let collector = Collector::new();

        let threads = (0..THREADS)
            .map(|_| {
                scoped::scope(|scope| {
                    scope.spawn(|| {
                        let handle = collector.handle();
                        for _ in 0..COUNT {
                            handle.pin(|scope| unsafe {
                                let a = Owned::new(Elem(7i32)).into_ptr(scope);
                                scope.defer(move || a.into_owned());
                            });
                        }
                    })
                })
            })
            .collect::<Vec<_>>();

        for t in threads {
            t.join();
        }

        let handle = collector.handle();
        while DROPS.load(Ordering::Relaxed) < COUNT * THREADS {
            handle.pin(|scope| collector.0.collect(scope));
        }
        assert_eq!(DROPS.load(Ordering::Relaxed), COUNT * THREADS);
    }
}
