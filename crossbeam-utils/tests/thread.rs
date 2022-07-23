use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;

use crossbeam_utils::thread;

const THREADS: usize = 10;
const SMALL_STACK_SIZE: usize = 20;

#[test]
fn join() {
    let counter = AtomicUsize::new(0);
    thread::scope(|scope| {
        let handle = scope.spawn(|_| {
            counter.store(1, Ordering::Relaxed);
        });
        assert!(handle.join().is_ok());

        let panic_handle = scope.spawn(|_| {
            panic!("\"My honey is running out!\", said Pooh.");
        });
        assert!(panic_handle.join().is_err());
    });

    // There should be sufficient synchronization.
    assert_eq!(1, counter.load(Ordering::Relaxed));
}

#[test]
fn counter() {
    let counter = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|_| {
                counter.fetch_add(1, Ordering::Relaxed);
            });
        }
    });

    assert_eq!(THREADS, counter.load(Ordering::Relaxed));
}

#[test]
fn counter_builder() {
    let counter = AtomicUsize::new(0);
    thread::scope(|scope| {
        for i in 0..THREADS {
            scope
                .builder()
                .name(format!("child-{}", i))
                .stack_size(SMALL_STACK_SIZE)
                .spawn(|_| {
                    counter.fetch_add(1, Ordering::Relaxed);
                })
                .unwrap();
        }
    });

    assert_eq!(THREADS, counter.load(Ordering::Relaxed));
}

#[test]
fn counter_panic() {
    let counter = AtomicUsize::new(0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        thread::scope(|scope| {
            scope.spawn(|_| {
                panic!("\"My honey is running out!\", said Pooh.");
            });
            sleep(Duration::from_millis(100));

            for _ in 0..THREADS {
                scope.spawn(|_| {
                    counter.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
    }));

    assert_eq!(THREADS, counter.load(Ordering::Relaxed));
    assert!(result.is_err());
}

#[test]
#[should_panic = "2 scoped thread(s) panicked"]
fn panic_twice() {
    thread::scope(|scope| {
        scope.spawn(|_| {
            sleep(Duration::from_millis(500));
            panic!("thread #1");
        });
        scope.spawn(|_| {
            panic!("thread #2");
        });
    });
}

#[test]
#[should_panic = "3 scoped thread(s) panicked"]
fn panic_many() {
    thread::scope(|scope| {
        scope.spawn(|_| panic!("deliberate panic #1"));
        scope.spawn(|_| panic!("deliberate panic #2"));
        scope.spawn(|_| panic!("deliberate panic #3"));
    });
}

#[test]
fn nesting() {
    let var = "foo".to_string();

    struct Wrapper<'a> {
        var: &'a String,
    }

    impl<'a> Wrapper<'a> {
        fn recurse(&'a self, scope: &thread::Scope<'a>, depth: usize) {
            assert_eq!(self.var, "foo");

            if depth > 0 {
                scope.spawn(move |scope| {
                    self.recurse(scope, depth - 1);
                });
            }
        }
    }

    let wrapper = Wrapper { var: &var };

    thread::scope(|scope| {
        scope.spawn(|scope| {
            scope.spawn(|scope| {
                wrapper.recurse(scope, 5);
            });
        });
    });
}

#[test]
fn join_nested() {
    thread::scope(|scope| {
        scope.spawn(|scope| {
            let handle = scope.spawn(|_| 7);

            sleep(Duration::from_millis(200));
            handle.join().unwrap();
        });

        sleep(Duration::from_millis(100));
    });
}

#[test]
fn scope_returns_ok() {
    let result = thread::scope(|scope| scope.spawn(|_| 1234).join().unwrap());
    assert_eq!(result, 1234);
}

#[cfg(unix)]
#[test]
fn as_pthread_t() {
    use std::os::unix::thread::JoinHandleExt;
    thread::scope(|scope| {
        let handle = scope.spawn(|_scope| {
            sleep(Duration::from_millis(100));
            42
        });
        let _pthread_t = handle.as_pthread_t();
        handle.join().unwrap();
    });
}

#[cfg(windows)]
#[test]
fn as_raw_handle() {
    use std::os::windows::io::AsRawHandle;
    thread::scope(|scope| {
        let handle = scope.spawn(|_scope| {
            sleep(Duration::from_millis(100));
            42
        });
        let _raw_handle = handle.as_raw_handle();
        handle.join().unwrap();
    });
}

// https://github.com/rust-lang/rust/pull/94644
// https://github.com/crossbeam-rs/crossbeam/pull/844#issuecomment-1143288972
#[test]
fn test_scoped_threads_drop_result_before_join() {
    let actually_finished = &AtomicBool::new(false);
    struct X<'a>(&'a AtomicBool);
    impl Drop for X<'_> {
        fn drop(&mut self) {
            std::thread::sleep(Duration::from_millis(100));
            self.0.store(true, Ordering::Relaxed);
        }
    }
    thread::scope(|s| {
        s.spawn(move |_| X(actually_finished));
    });
    assert!(actually_finished.load(Ordering::Relaxed));
}
