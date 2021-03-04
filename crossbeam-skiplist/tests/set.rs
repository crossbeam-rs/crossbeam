use crossbeam_skiplist::SkipSet;
use crossbeam_utils::thread;
use std::{iter, sync::Barrier};

#[test]
fn smoke() {
    let m = SkipSet::new();
    m.insert(1);
    m.insert(5);
    m.insert(7);
}

// https://github.com/crossbeam-rs/crossbeam/issues/672
#[test]
fn concurrent_insert() {
    for _ in 0..100 {
        let set: SkipSet<i32> = iter::once(1).collect();
        let barrier = Barrier::new(2);
        thread::scope(|s| {
            s.spawn(|_| {
                barrier.wait();
                set.insert(1);
            });
            s.spawn(|_| {
                barrier.wait();
                set.insert(1);
            });
        })
        .unwrap();
    }
}

// https://github.com/crossbeam-rs/crossbeam/issues/672
#[test]
fn concurrent_remove() {
    for _ in 0..100 {
        let set: SkipSet<i32> = iter::once(1).collect();
        let barrier = Barrier::new(2);
        thread::scope(|s| {
            s.spawn(|_| {
                barrier.wait();
                set.remove(&1);
            });
            s.spawn(|_| {
                barrier.wait();
                set.remove(&1);
            });
        })
        .unwrap();
    }
}

#[test]
fn iter() {
    let s = SkipSet::new();
    for &x in &[4, 2, 12, 8, 7, 11, 5] {
        s.insert(x);
    }

    assert_eq!(
        s.iter().map(|e| *e).collect::<Vec<_>>(),
        &[2, 4, 5, 7, 8, 11, 12]
    );

    let mut it = s.iter();
    s.remove(&2);
    assert_eq!(*it.next().unwrap(), 4);
    s.remove(&7);
    assert_eq!(*it.next().unwrap(), 5);
    s.remove(&5);
    assert_eq!(*it.next().unwrap(), 8);
    s.remove(&12);
    assert_eq!(*it.next().unwrap(), 11);
    assert!(it.next().is_none());
}

// https://github.com/crossbeam-rs/crossbeam/issues/671
#[test]
fn iter_range() {
    let set: SkipSet<_> = [1, 3, 5].iter().cloned().collect();
    assert_eq!(set.range(2..4).count(), 1);
    set.insert(3);
}
