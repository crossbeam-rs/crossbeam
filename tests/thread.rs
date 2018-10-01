extern crate crossbeam_utils;

use std::any::Any;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;

use crossbeam_utils::thread;

const THREADS: usize = 10;
const SMALL_STACK_SIZE: usize = 20;

#[test]
fn join() {
    let counter = AtomicUsize::new(0);
    thread::scope(|scope| {
        let handle = scope.spawn(|| {
            counter.store(1, Ordering::Relaxed);
        });
        assert!(handle.join().is_ok());

        let panic_handle = scope.spawn(|| {
            panic!("\"My honey is running out!\", said Pooh.");
        });
        assert!(panic_handle.join().is_err());
    }).unwrap();

    // There should be sufficient synchronization.
    assert_eq!(1, counter.load(Ordering::Relaxed));
}

#[test]
fn counter() {
    let counter = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                counter.fetch_add(1, Ordering::Relaxed);
            });
        }
    }).unwrap();

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
                .spawn(|| {
                    counter.fetch_add(1, Ordering::Relaxed);
                })
                .unwrap();
        }
    }).unwrap();

    assert_eq!(THREADS, counter.load(Ordering::Relaxed));
}

#[test]
fn counter_panic() {
    let counter = AtomicUsize::new(0);
    let result = thread::scope(|scope| {
        scope.spawn(|| {
            panic!("\"My honey is running out!\", said Pooh.");
        });
        sleep(Duration::from_millis(100));

        for _ in 0..THREADS {
            scope.spawn(|| {
                counter.fetch_add(1, Ordering::Relaxed);
            });
        }
    });

    assert_eq!(THREADS, counter.load(Ordering::Relaxed));
    assert!(result.is_err());
}

#[test]
fn panic_twice() {
    let result = thread::scope(|scope| {
        scope.spawn(|| {
            panic!("thread");
        });
        panic!("scope");
    });

    let err = result.unwrap_err();
    let vec = err.downcast_ref::<Vec<Box<Any + Send + 'static>>>().unwrap();
    assert_eq!(2, vec.len());

    let first = vec[0].downcast_ref::<&str>().unwrap();
    let second = vec[1].downcast_ref::<&str>().unwrap();
    assert_eq!("scope", *first);
    assert_eq!("thread", *second)
}

#[test]
fn panic_many() {
    let result = thread::scope(|scope| {
        scope.spawn(|| panic!("deliberate panic #1"));
        scope.spawn(|| panic!("deliberate panic #2"));
        scope.spawn(|| panic!("deliberate panic #3"));
    });

    let err = result.unwrap_err();
    let vec = err
        .downcast_ref::<Vec<Box<Any + Send + 'static>>>()
        .unwrap();
    assert_eq!(3, vec.len());

    for panic in vec.iter() {
        let panic = panic.downcast_ref::<&str>().unwrap();
        assert!(
            *panic == "deliberate panic #1"
                || *panic == "deliberate panic #2"
                || *panic == "deliberate panic #3"
        );
    }
}
