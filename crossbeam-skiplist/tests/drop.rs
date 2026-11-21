use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crossbeam_epoch::Collector;
use crossbeam_skiplist::{
    SkipList,
    comparator::{Comparator, Equivalator},
};

struct Value {
    id: usize,
    drops: Arc<Vec<AtomicUsize>>,
}

impl Drop for Value {
    fn drop(&mut self) {
        assert_eq!(self.drops[self.id].fetch_add(1, Ordering::Relaxed), 0);
    }
}

fn counters(len: usize) -> Arc<Vec<AtomicUsize>> {
    Arc::new((0..len).map(|_| AtomicUsize::new(0)).collect())
}

fn assert_all_dropped(drops: &[AtomicUsize]) {
    for (id, count) in drops.iter().enumerate() {
        assert_eq!(
            count.load(Ordering::Relaxed),
            1,
            "value {id} was not dropped"
        );
    }
}

// On 64-bit targets, insertion 6 has height 2, insertion 7 has height 3,
// and insertion 8 has height 1. Replacing key 1 and iterating can unlink
// insertion 6 at level 0 while leaving it linked above level 0.
const KEYS: [u64; 9] = [0, 0, 0, 0, 0, 0, 1, 0, 1];

fn check_drop(keys: &[u64], iterate: bool, clear: bool, forget: Option<usize>) {
    let drops = counters(keys.len());
    // A private collector isolates reclamation from other tests' guards.
    // Destroy it before checking counts, so deferred destructors also run.
    {
        let collector = Collector::new();
        let handle = collector.register();
        let mut guard = handle.pin();
        let list = SkipList::new(collector.clone());
        list.clear(&mut guard);
        for (id, &key) in keys.iter().enumerate() {
            let entry = list.insert(
                key,
                Value {
                    id,
                    drops: drops.clone(),
                },
                &guard,
            );
            // RefEntry has no Drop implementation; skipping release leaves the
            // same outstanding reference as forgetting the entry.
            if forget != Some(id) {
                entry.release(&guard);
            }
        }
        if iterate {
            for entry in list.iter(&guard) {
                let _ = entry.key();
            }
        }
        if clear {
            list.clear(&mut guard);
        }
        drop(list);
    }
    assert_all_dropped(&drops);
}

#[test]
fn drop_nodes_linked_only_above_level_zero() {
    check_drop(&KEYS, true, false, None);
    check_drop(&KEYS, true, true, None);
    check_drop(&KEYS, false, true, None);
}

#[test]
fn drop_with_forgotten_entries() {
    check_drop(&[0], false, false, Some(0));
    check_drop(&KEYS, true, false, Some(6));
}

#[test]
fn drop_at_clear_batch_boundaries() {
    for len in [0, 1, 99, 100, 101, 199, 200, 201] {
        let keys: Vec<_> = (0..len).chain(0..len).collect();
        check_drop(&keys, true, true, None);
    }
}

#[test]
fn drop_after_partial_tower_panic() {
    struct PanicOnEquality(Arc<AtomicBool>);

    impl Equivalator<u64> for PanicOnEquality {
        fn equivalent(&self, lhs: &u64, rhs: &u64) -> bool {
            assert!(!self.0.load(Ordering::Relaxed), "upper-tower comparison");
            lhs == rhs
        }
    }

    impl Comparator<u64> for PanicOnEquality {
        fn compare(&self, lhs: &u64, rhs: &u64) -> std::cmp::Ordering {
            lhs.cmp(rhs)
        }
    }

    let drops = counters(8);
    {
        let collector = Collector::new();
        let handle = collector.register();
        let guard = handle.pin();
        let panic = Arc::new(AtomicBool::new(false));
        let list = SkipList::with_comparator(collector.clone(), PanicOnEquality(panic.clone()));
        for (id, &key) in KEYS[..7].iter().enumerate() {
            list.insert(
                key,
                Value {
                    id,
                    drops: drops.clone(),
                },
                &guard,
            )
            .release(&guard);
        }

        panic.store(true, Ordering::Relaxed);
        let result = catch_unwind(AssertUnwindSafe(|| {
            list.insert(
                0,
                Value {
                    id: 7,
                    drops: drops.clone(),
                },
                &guard,
            )
            .release(&guard);
        }));
        panic.store(false, Ordering::Relaxed);
        assert!(result.is_err());
        // The node was published at level 0 before building its upper levels
        // panicked, leaving the unreleased return entry's reference behind.
        assert_eq!(list.get(&0, &guard).unwrap().value().id, 7);
        drop(list);
    }
    assert_all_dropped(&drops);
}

#[test]
fn drop_after_concurrent_operations() {
    let per_thread = if cfg!(miri) { 12 } else { 128 };
    let drops = counters(4 * per_thread);
    {
        let collector = Collector::new();
        let list = SkipList::new(collector.clone());
        crossbeam_utils::thread::scope(|scope| {
            for thread in 0..4 {
                let list = &list;
                let collector = &collector;
                let drops = &drops;
                scope.spawn(move |_| {
                    let handle = collector.register();
                    let mut guard = handle.pin();
                    for i in 0..per_thread {
                        let id = thread * per_thread + i;
                        list.insert(
                            (id % 11) as u64,
                            Value {
                                id,
                                drops: drops.clone(),
                            },
                            &guard,
                        )
                        .release(&guard);
                        if i % 3 == 0 {
                            for entry in list.iter(&guard) {
                                let _ = entry.key();
                            }
                        }
                        if i % 7 == 0 {
                            list.clear(&mut guard);
                        }
                    }
                });
            }
        })
        .unwrap();
        drop(list);
    }
    assert_all_dropped(&drops);
}
