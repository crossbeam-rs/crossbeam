// Put in module instead of using #![cfg(..)] to work around rustc/cargo bug around -Z crate-attr.
#[cfg(crossbeam_loom)]
mod tests {
    use std::{mem::ManuallyDrop, ptr};

    use crossbeam_epoch as epoch;
    use epoch::{Atomic, Owned, *};
    use loom::{
        sync::{
            Arc,
            atomic::Ordering::{self, Acquire, Relaxed, Release},
        },
        thread::spawn,
    };
    use loom_crate as loom;

    #[test]
    fn it_works() {
        loom::model(|| {
            let collector = Collector::new();
            let item: Atomic<String> = Atomic::from(Owned::new(String::from("boom")));
            let item2 = item.clone();
            let collector2 = collector.clone();
            let guard = collector.register().pin();

            let jh = loom::thread::spawn(move || {
                let guard = collector2.register().pin();
                guard.defer(move || {
                    // this isn't really safe, since other threads may still have pointers to the
                    // value, but in this limited test scenario it's okay, since we know the test won't
                    // access item after all the pins are released.
                    let mut item = unsafe { item2.into_owned() };
                    // mutate it as a second measure to make sure the assert_eq below would fail
                    item.retain(|c| c == 'o');
                    drop(item);
                });
            });

            let item = item.load(Ordering::SeqCst, &guard);
            // we pinned strictly before the call to defer_destroy,
            // so item cannot have been dropped yet
            assert_eq!(*unsafe { item.deref() }, "boom");
            drop(guard);

            jh.join().unwrap();

            drop(collector);
        })
    }

    #[test]
    fn treiber_stack() {
        /// Treiber's lock-free stack.
        ///
        /// Usable with any number of producers and consumers.
        #[derive(Debug)]
        struct TreiberStack<T> {
            head: Atomic<Node<T>>,
        }

        #[derive(Debug)]
        struct Node<T> {
            data: ManuallyDrop<T>,
            next: Atomic<Node<T>>,
        }

        impl<T> TreiberStack<T> {
            /// Creates a new, empty stack.
            fn new() -> Self {
                Self {
                    head: Atomic::null(),
                }
            }

            /// Pushes a value on top of the stack.
            fn push(&self, t: T) {
                let mut n = Owned::new(Node {
                    data: ManuallyDrop::new(t),
                    next: Atomic::null(),
                });

                let guard = epoch::pin();

                loop {
                    let head = self.head.load(Relaxed, &guard);
                    n.next.store(head, Relaxed);

                    match self
                        .head
                        .compare_exchange(head, n, Release, Relaxed, &guard)
                    {
                        Ok(_) => break,
                        Err(e) => n = e.new,
                    }
                }
            }

            /// Attempts to pop the top element from the stack.
            ///
            /// Returns `None` if the stack is empty.
            fn pop(&self) -> Option<T> {
                let guard = epoch::pin();
                loop {
                    let head = self.head.load(Acquire, &guard);

                    match unsafe { head.as_ref() } {
                        Some(h) => {
                            let next = h.next.load(Relaxed, &guard);

                            if self
                                .head
                                .compare_exchange(head, next, Relaxed, Relaxed, &guard)
                                .is_ok()
                            {
                                unsafe {
                                    guard.defer_destroy(head);
                                    return Some(ManuallyDrop::into_inner(ptr::read(&(*h).data)));
                                }
                            }
                        }
                        None => return None,
                    }
                }
            }

            /// Returns `true` if the stack is empty.
            fn is_empty(&self) -> bool {
                let guard = epoch::pin();
                self.head.load(Acquire, &guard).is_null()
            }
        }

        impl<T> Drop for TreiberStack<T> {
            fn drop(&mut self) {
                while self.pop().is_some() {}
            }
        }

        loom::model(|| {
            let stack1 = Arc::new(TreiberStack::new());
            let stack2 = Arc::clone(&stack1);

            // use 5 since it's greater than the 4 used for the sanitize feature
            let jh = spawn(move || {
                for i in 0..5 {
                    stack2.push(i);
                    assert!(stack2.pop().is_some());
                }
            });

            for i in 0..5 {
                stack1.push(i);
                assert!(stack1.pop().is_some());
            }

            jh.join().unwrap();
            assert!(stack1.pop().is_none());
            assert!(stack1.is_empty());
        });
    }

    /// Regression test for https://github.com/crossbeam-rs/crossbeam/issues/1352.
    ///
    /// On non-x86 targets, `Local::pin` used to publish the new epoch with a `Relaxed`
    /// store followed by a `SeqCst` fence. That does not order the previous critical
    /// section's accesses before reclamation: since C++20, release sequences no longer
    /// include same-thread plain stores, so `try_advance`'s `Acquire` fence could observe
    /// the new epoch without synchronizing with the previous unpin, letting a deferred
    /// destructor run while the earlier critical section's accesses were still in flight.
    #[test]
    fn pin_orders_previous_critical_section_before_reclamation() {
        use loom::{cell::UnsafeCell, sync::atomic::AtomicBool};

        loom::model(|| {
            let collector = Collector::new();
            let cell = Arc::new(UnsafeCell::new(7));
            let linked = Arc::new(AtomicBool::new(true));

            let h = collector.register();
            let (c, l) = (cell.clone(), linked.clone());
            let reader = spawn(move || {
                for _ in 0..2 {
                    let g = h.pin();
                    if l.load(Acquire) {
                        c.with(|v| unsafe { assert_eq!(*v, 7) });
                    }
                    drop(g);
                }
            });

            let h = collector.register();
            linked.store(false, Release);
            {
                let g = h.pin();
                let c = cell.clone();
                unsafe { g.defer_unchecked(move || c.with_mut(|v| *v = 0)) };
                g.flush();
            }
            for _ in 0..3 {
                h.pin().flush();
            }

            reader.join().unwrap();
        });
    }
}
