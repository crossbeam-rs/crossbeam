use std::collections::VecDeque;
use std::mem;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use crossbeam_queue::BatchQueue;
use crossbeam_utils::thread::scope;

fn pop_exact(queue: &BatchQueue<usize>, requested: usize, expected: usize) -> Vec<usize> {
    let batch = queue.pop(requested);
    let actual = batch.len();

    // Do not run a malformed iterator's Drop implementation. In addition to making the
    // length assertion less informative, doing so could access slots that were not reserved.
    if actual != expected {
        mem::forget(batch);
        panic!("requested {requested} items and expected {expected}, but got {actual}");
    }

    batch.collect()
}

#[test]
fn empty_pop_returns_immediately() {
    let queue = BatchQueue::<usize>::new();
    assert_eq!(queue.pop(8).len(), 0);
}

#[test]
fn empty_push_returns_immediately() {
    BatchQueue::new().push(Vec::<usize>::new());
}

#[test]
fn zero_length_pop_is_a_noop() {
    let queue = BatchQueue::new();

    queue.push([10, 11, 12]);

    assert!(pop_exact(&queue, 0, 0).is_empty());
    assert_eq!(pop_exact(&queue, 3, 3), [10, 11, 12]);
}

#[test]
fn fifo_with_mixed_batch_sizes() {
    let queue = BatchQueue::new();
    let push_sizes = [1, 5, 2, 9, 3, 7];
    let pop_sizes = [4, 1, 8, 3, 20];
    let total = push_sizes.iter().sum();
    let mut next = 0;

    for size in push_sizes {
        queue.push(next..next + size);
        next += size;
    }

    let mut values = Vec::new();
    for requested in pop_sizes {
        let expected = requested.min(total - values.len());
        values.extend(pop_exact(&queue, requested, expected));
    }

    assert_eq!(values, (0..total).collect::<Vec<_>>());
    assert_eq!(queue.pop(1).len(), 0);
}

#[test]
fn mixed_batches_match_a_sequential_queue_model() {
    const STEPS: usize = if cfg!(miri) { 100 } else { 5_000 };

    let queue = BatchQueue::new();
    let mut expected = VecDeque::new();
    let mut random = 0x4d59_5df4_d0f3_3173_u64;
    let mut next_value = 0;

    for _ in 0..STEPS {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);

        if expected.is_empty() || random % 3 != 0 {
            let len = (random % 70) as usize;
            let end = next_value + len;
            queue.push(next_value..end);
            expected.extend(next_value..end);
            next_value = end;
        } else {
            let requested = (random % 80) as usize;
            let expected_len = requested.min(expected.len());
            let actual = pop_exact(&queue, requested, expected_len);
            let model = expected.drain(..expected_len).collect::<Vec<_>>();
            assert_eq!(actual, model);
        }
    }

    while !expected.is_empty() {
        let expected_len = 37.min(expected.len());
        let actual = pop_exact(&queue, 37, expected_len);
        let model = expected.drain(..expected_len).collect::<Vec<_>>();
        assert_eq!(actual, model);
    }

    assert_eq!(queue.pop(37).len(), 0);
}

#[test]
fn pop_larger_than_the_available_items() {
    let queue = BatchQueue::new();
    queue.push(20..25);

    assert_eq!(pop_exact(&queue, usize::MAX, 5), [20, 21, 22, 23, 24]);
    assert_eq!(queue.pop(usize::MAX).len(), 0);
}

#[test]
fn batches_cross_internal_block_boundaries() {
    let queue = BatchQueue::new();
    let push_sizes = [30, 2, 31, 33, 1, 62];
    let pop_sizes = [1, 30, 2, 29, 32, 7, 58];
    let total = push_sizes.iter().sum();
    let mut next = 0;

    for size in push_sizes {
        queue.push(next..next + size);
        next += size;
    }

    let mut values = Vec::with_capacity(total);
    for requested in pop_sizes {
        let expected = requested.min(total - values.len());
        values.extend(pop_exact(&queue, requested, expected));
    }

    assert_eq!(values, (0..total).collect::<Vec<_>>());
}

#[test]
fn one_batch_can_match_or_exceed_an_internal_block() {
    for size in [31, 32, 63, 64, 257] {
        let queue = BatchQueue::new();
        queue.push(0..size);

        assert_eq!(pop_exact(&queue, size, size), (0..size).collect::<Vec<_>>());
    }
}

#[test]
#[cfg(not(miri))]
fn one_batch_can_exceed_the_position_field() {
    let len = usize::from(u16::MAX) + 1;
    let queue = BatchQueue::new();
    queue.push(0..len);

    assert_eq!(pop_exact(&queue, len, len), (0..len).collect::<Vec<_>>());
}

#[test]
fn a_push_can_exactly_fill_the_current_block() {
    let queue = BatchQueue::new();
    queue.push([0]);
    queue.push(1..31);

    assert_eq!(pop_exact(&queue, 31, 31), (0..31).collect::<Vec<_>>());
}

#[test]
fn iterator_reports_the_reserved_length() {
    let queue = BatchQueue::new();
    queue.push(0..8);

    let mut batch = queue.pop(5);
    let actual = batch.len();
    if actual != 5 {
        mem::forget(batch);
        panic!("expected a five-item batch, but got {actual}");
    }

    assert_eq!(batch.size_hint(), (5, Some(5)));
    assert_eq!(batch.next(), Some(0));
    assert_eq!(batch.len(), 4);
    assert_eq!(batch.size_hint(), (4, Some(4)));
    assert_eq!(batch.collect::<Vec<_>>(), [1, 2, 3, 4]);

    assert_eq!(pop_exact(&queue, 8, 3), [5, 6, 7]);
}

#[test]
fn dropping_an_iterator_discards_the_rest_of_its_batch() {
    let queue = BatchQueue::new();
    queue.push(0..10);

    let mut batch = queue.pop(6);
    let actual = batch.len();
    if actual != 6 {
        mem::forget(batch);
        panic!("expected a six-item batch, but got {actual}");
    }

    assert_eq!(batch.next(), Some(0));
    assert_eq!(batch.next(), Some(1));
    drop(batch);

    assert_eq!(pop_exact(&queue, 10, 4), [6, 7, 8, 9]);
}

#[test]
fn dropping_the_queue_drops_unconsumed_items() {
    struct DropCounter<'a>(&'a AtomicUsize);

    impl Drop for DropCounter<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drops = AtomicUsize::new(0);
    let queue = BatchQueue::new();
    queue.push((0..97).map(|_| DropCounter(&drops)));

    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(queue);
    assert_eq!(drops.load(Ordering::SeqCst), 97);
}

#[test]
fn an_iterator_can_outlive_the_queue() {
    struct DropCounter<'a> {
        value: usize,
        drops: &'a AtomicUsize,
    }

    impl Drop for DropCounter<'_> {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drops = AtomicUsize::new(0);
    let batch = {
        let queue = BatchQueue::new();
        queue.push((0..5).map(|value| DropCounter {
            value,
            drops: &drops,
        }));
        queue.pop(3)
    };

    assert_eq!(batch.map(|item| item.value).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(drops.load(Ordering::SeqCst), 5);
}

#[test]
fn dropping_a_partially_consumed_multiblock_queue_drops_every_item() {
    struct DropCounter<'a>(&'a AtomicUsize);

    impl Drop for DropCounter<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drops = AtomicUsize::new(0);
    let queue = BatchQueue::new();
    queue.push((0..30).map(|_| DropCounter(&drops)));
    queue.push((30..70).map(|_| DropCounter(&drops)));

    drop(queue.pop(1).next().unwrap());
    assert_eq!(drops.load(Ordering::SeqCst), 1);

    drop(queue);
    assert_eq!(drops.load(Ordering::SeqCst), 70);
}

#[test]
fn multiple_producers_preserve_each_producers_order() {
    const PRODUCERS: usize = 4;
    const ITEMS_PER_PRODUCER: usize = if cfg!(miri) { 64 } else { 2_000 };
    const PUSH_SIZES: [usize; 7] = [1, 7, 2, 31, 5, 64, 13];
    const POP_SIZES: [usize; 6] = [3, 1, 29, 8, 47, 2];

    let queue = BatchQueue::new();

    scope(|scope| {
        for producer in 0..PRODUCERS {
            let queue = &queue;
            scope.spawn(move |_| {
                let mut sequence = 0;
                let mut batch = 0;

                while sequence < ITEMS_PER_PRODUCER {
                    let len =
                        PUSH_SIZES[batch % PUSH_SIZES.len()].min(ITEMS_PER_PRODUCER - sequence);
                    queue.push(
                        (sequence..sequence + len)
                            .map(|sequence| producer * ITEMS_PER_PRODUCER + sequence),
                    );
                    sequence += len;
                    batch += 1;
                }
            });
        }
    })
    .unwrap();

    let total = PRODUCERS * ITEMS_PER_PRODUCER;
    let mut values = Vec::with_capacity(total);
    let mut batch = 0;

    while values.len() < total {
        let requested = POP_SIZES[batch % POP_SIZES.len()];
        let expected = requested.min(total - values.len());
        values.extend(pop_exact(&queue, requested, expected));
        batch += 1;
    }

    let mut next = [0; PRODUCERS];
    for value in values {
        let producer = value / ITEMS_PER_PRODUCER;
        let sequence = value % ITEMS_PER_PRODUCER;

        assert!(producer < PRODUCERS, "out-of-range value {value}");
        assert_eq!(
            sequence, next[producer],
            "producer {producer} was reordered"
        );
        next[producer] += 1;
    }
    assert_eq!(next, [ITEMS_PER_PRODUCER; PRODUCERS]);
}

#[test]
fn multiple_consumers_receive_every_item_exactly_once() {
    const CONSUMERS: usize = 4;
    const ROUNDS: usize = if cfg!(miri) { 4 } else { 100 };
    const REQUESTS: [usize; 5] = [1, 3, 7, 16, 33];

    let total = (0..CONSUMERS)
        .flat_map(|consumer| {
            (0..ROUNDS).map(move |round| REQUESTS[(consumer + round) % REQUESTS.len()])
        })
        .sum();
    let queue = BatchQueue::new();
    let mut start = 0;

    for size in [2, 31, 5, 64, 1, 19].iter().copied().cycle() {
        if start == total {
            break;
        }
        let end = (start + size).min(total);
        queue.push(start..end);
        start = end;
    }

    let counts = (0..total).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();
    let malformed_batch = AtomicBool::new(false);

    scope(|scope| {
        for consumer in 0..CONSUMERS {
            let queue = &queue;
            let counts = &counts;
            let malformed_batch = &malformed_batch;

            scope.spawn(move |_| {
                for round in 0..ROUNDS {
                    if malformed_batch.load(Ordering::Acquire) {
                        return;
                    }

                    let requested = REQUESTS[(consumer + round) % REQUESTS.len()];
                    let mut batch = queue.pop(requested);
                    if batch.len() != requested {
                        mem::forget(batch);
                        malformed_batch.store(true, Ordering::Release);
                        return;
                    }

                    while let Some(value) = batch.next() {
                        if let Some(count) = counts.get(value) {
                            count.fetch_add(1, Ordering::SeqCst);
                        } else {
                            mem::forget(batch);
                            malformed_batch.store(true, Ordering::Release);
                            return;
                        }
                    }
                }
            });
        }
    })
    .unwrap();

    assert!(
        !malformed_batch.load(Ordering::Acquire),
        "a pop returned the wrong batch length or an out-of-range item"
    );
    for (value, count) in counts.iter().enumerate() {
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "item {value} was lost or duplicated"
        );
    }
}

#[test]
fn mixed_size_mpmc_workload_delivers_every_item_exactly_once() {
    const PRODUCERS: usize = 4;
    const CONSUMERS: usize = 4;
    const ITEMS_PER_PRODUCER: usize = if cfg!(miri) { 64 } else { 4_000 };
    const PREFILL: usize = 64;
    const PUSH_SIZES: [usize; 7] = [1, 17, 3, 64, 2, 31, 9];
    const POP_SIZES: [usize; 7] = [7, 1, 64, 3, 97, 2, 33];

    let total = PRODUCERS * ITEMS_PER_PRODUCER;
    let queue = BatchQueue::new();
    queue.push(0..PREFILL);

    let counts = (0..total).map(|_| AtomicUsize::new(0)).collect::<Vec<_>>();
    let consumed = AtomicUsize::new(0);
    let malformed_batch = AtomicBool::new(false);
    let start = Barrier::new(PRODUCERS + CONSUMERS);

    scope(|scope| {
        for producer in 0..PRODUCERS {
            let queue = &queue;
            let malformed_batch = &malformed_batch;
            let start = &start;

            scope.spawn(move |_| {
                start.wait();

                let mut sequence = if producer == 0 { PREFILL } else { 0 };
                let mut batch = 0;
                while sequence < ITEMS_PER_PRODUCER {
                    if malformed_batch.load(Ordering::Acquire) {
                        return;
                    }

                    let len = PUSH_SIZES[(producer + batch) % PUSH_SIZES.len()]
                        .min(ITEMS_PER_PRODUCER - sequence);
                    queue.push(
                        (sequence..sequence + len)
                            .map(|sequence| producer * ITEMS_PER_PRODUCER + sequence),
                    );
                    sequence += len;
                    batch += 1;
                }
            });
        }

        for consumer in 0..CONSUMERS {
            let queue = &queue;
            let counts = &counts;
            let consumed = &consumed;
            let malformed_batch = &malformed_batch;
            let start = &start;

            scope.spawn(move |_| {
                start.wait();

                let mut batch_number = 0;
                while consumed.load(Ordering::Acquire) < total {
                    if malformed_batch.load(Ordering::Acquire) {
                        return;
                    }

                    let requested = POP_SIZES[(consumer + batch_number) % POP_SIZES.len()];
                    let mut batch = queue.pop(requested);
                    let len = batch.len();
                    if len > requested {
                        mem::forget(batch);
                        malformed_batch.store(true, Ordering::Release);
                        return;
                    }

                    if len == 0 {
                        thread::yield_now();
                        continue;
                    }

                    while let Some(value) = batch.next() {
                        let count = match counts.get(value) {
                            Some(count) => count,
                            None => {
                                mem::forget(batch);
                                malformed_batch.store(true, Ordering::Release);
                                return;
                            }
                        };
                        count.fetch_add(1, Ordering::SeqCst);
                        consumed.fetch_add(1, Ordering::Release);
                    }
                    batch_number += 1;
                }
            });
        }
    })
    .unwrap();

    assert!(
        !malformed_batch.load(Ordering::Acquire),
        "a pop exceeded its requested size or returned an out-of-range item"
    );
    assert_eq!(consumed.load(Ordering::Acquire), total);
    for (value, count) in counts.iter().enumerate() {
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "item {value} was lost or duplicated"
        );
    }
}
