//! Unrolled Michael—Scott queues.

use std::cell::UnsafeCell;
use std::ops::Range;
use std::sync::atomic::{self, AtomicBool, AtomicUsize};
use std::{ptr, mem, fmt, cmp};

use epoch::{self, Atomic};

/// The maximal number of entries in a segment.
const SEG_SIZE: usize = 32;

/// A segment.
///
/// Segments are like a slab of items stored all in one node, such that queing and dequing can be
/// done faster.
struct Segment<T> {
    /// The used entries in the segment.
    used: Range<AtomicUsize>,
    /// The data.
    data: [UnsafeCell<T>; SEG_SIZE],
    /// What entries contain valid data?
    valid: [AtomicBool; SEG_SIZE],
    /// The next node.
    next: Atomic<Segment<T>>,
}

impl<T> Default for Segment<T> {
    fn default() -> Segment<T> {
        Segment {
            used: AtomicUsize::new(0)..AtomicUsize::new(0),
            data: unsafe { mem::uninitialized() },
            // FIXME: This is currently needed due to `AtomicBool` not implementing `Copy`.
            valid: unsafe { mem::zeroed() },
            next: Atomic::null(),
        }
    }
}

impl<T> fmt::Debug for Segment<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Segment {{ ... }}")
    }
}

unsafe impl<T: Send> Sync for Segment<T> {}

/// An faster, unrolled Michael-Scott queue.
///
/// This queue, usable with any number of producers and consumers, stores so called segments
/// (arrays of nodes) in order to be more lightweight.
#[derive(Debug)]
pub struct SegQueue<T> {
    /// The head segment.
    head: Atomic<Segment<T>>,
    /// The tail segment.
    tail: Atomic<Segment<T>>,
}

impl<T> SegQueue<T> {
    /// Create a new, empty queue.
    pub fn new() -> SegQueue<T> {
        // FIXME: This is ugly.

        // We start setting the two ends to null pointers.
        let queue = SegQueue {
            head: Atomic::null(),
            tail: Atomic::null(),
        };

        // Construct the sentinel node with an empty segment.
        let sentinel = Box::new(Segment::default());

        // Set the two ends to the sentinel node.
        let guard = epoch::pin();
        let sentinel = queue.head.store_and_ref(sentinel, atomic::Ordering::Relaxed, &guard);
        queue.tail.store_shared(Some(sentinel), atomic::Ordering::Relaxed);

        queue
    }

    /// Push an element to the back of the queue.
    pub fn queue(&self, elem: T) {
        // Pin the epoch.
        let guard = epoch::pin();

        loop {
            // Load the tail of the queue through the epoch.
            let tail = self.tail.load(atomic::Ordering::Acquire, &guard).unwrap();
            // Check if the segment is full.
            if tail.used.end.load(atomic::Ordering::Relaxed) >= SEG_SIZE {
                // Segment full. Spin again (the thread exhausting the segment is responsible for
                // adding a new node).
                continue;
            }

            // Increment the used range to ensure that another thread doesn't write the cell, we'll
            // use soon.
            let i = tail.used.end.fetch_add(1, atomic::Ordering::Relaxed);

            // If and only if the increment range was within the segment size (i.e. that we did
            // actually get a spare cell), we will write the data. Otherwise, we will spin.
            if i >= SEG_SIZE {
                continue;
            }

            // Write that data.
            unsafe {
                // Obtain the cell.
                let cell = (*tail).data[i].get();
                // Write the data into the cell.
                ptr::write(&mut *cell, elem);
                // Now that the cell is initialized, we can set the valid flag.
                tail.valid[i].store(true, atomic::Ordering::Release);

                // Handle end-of-segment.
                if i + 1 == SEG_SIZE {
                    // As we noted before in this function, the thread which exhausts the segment
                    // is responsible for inserting a new node, and that's us.

                    // Append a new segment to the local tail.
                    let tail = tail.next
                        .store_and_ref(Box::new(Segment::default()), atomic::Ordering::Release, &guard);
                    // Store the local tail to the queue.
                    self.tail.store_shared(Some(tail), atomic::Ordering::Release);
                }

                // It was queued; break the loop.
                break;
            }
        }
    }

    /// Attempt to dequeue from the front.
    ///
    /// This returns `None` if the queue is observed to be empty.
    pub fn dequeue(&self) -> Option<T> {
        // Pin the epoch.
        let guard = epoch::pin();

        // Spin until the other thread which is currently modfying the queue leaves it in a
        // dequeue-able state.
        loop {
            // Load the head through the guard.
            let head = self.head.load(atomic::Ordering::Acquire, &guard).unwrap();

            // Spin until the ABA condition is solved.
            loop {
                // Load the lower bound for the usable entries.
                let low = head.used.start.load(atomic::Ordering::Relaxed);
                // Ensure that the usable range is non-empty.
                if low >= cmp::min(head.used.end.load(atomic::Ordering::Relaxed), SEG_SIZE) {
                    // Since the range is either empty or out-of-segment, we must wait for the
                    // responsible thread to insert a new node.
                    break;
                }

                if head.used.start.compare_and_swap(low, low + 1, atomic::Ordering::Relaxed) == low {
                    unsafe {
                        // Read the cell.
                        let ret = ptr::read((*head).data[low].get());

                        // Spin until the cell's data is valid.
                        while !head.valid[low].load(atomic::Ordering::Acquire) {}

                        // If all the elements in this segment have been dequeued, we must go to
                        // the next segment.
                        if low + 1 == SEG_SIZE {
                            // Spin until the other thread, responsible for adding the new segment,
                            // completes its job.
                            loop {
                                // Check if there is another node.
                                if let Some(next) = head.next.load(atomic::Ordering::Acquire, &guard) {
                                    // Another node were found. As the current segment is dead,
                                    // update the head node.
                                    self.head.store_shared(Some(next), atomic::Ordering::Release);
                                    // Unlink the old head node.
                                    guard.unlinked(head);

                                    // Break to the return statement.
                                    break;
                                }
                            }
                        }

                        // Finally return.
                        return Some(ret);
                    }
                }
            }

            // Check if this is the last node and the segment is dead.
            if head.next.load(atomic::Ordering::Relaxed, &guard).is_none() {
                // No more nodes.
                return None;
            }
        }
    }
}

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use super::*;
    use std::thread;
    use std::sync::Arc;

    #[test]
    fn queue_dequeue_1() {
        let q: SegQueue<i64> = SegQueue::new();
        q.queue(37);
        assert_eq!(q.dequeue(), Some(37));
    }

    #[test]
    fn queue_dequeue_2() {
        let q: SegQueue<i64> = SegQueue::new();
        q.queue(37);
        q.queue(48);
        assert_eq!(q.dequeue(), Some(37));
        assert_eq!(q.dequeue(), Some(48));
    }

    #[test]
    fn queue_dequeue_many_seq() {
        let q: SegQueue<i64> = SegQueue::new();
        for i in 0..200 {
            q.queue(i)
        }
        for i in 0..200 {
            assert_eq!(q.dequeue(), Some(i));
        }
    }

    #[test]
    fn queue_dequeue_many_spsc() {
        let q: Arc<SegQueue<i64>> = Arc::new(SegQueue::new());

        let qr = q.clone();
        let join = thread::spawn(move || {
            let mut next = 0;

            while next < CONC_COUNT {
                if let Some(elem) = qr.dequeue() {
                    assert_eq!(elem, next);
                    next += 1;
                }
            }
        });

        for i in 0..CONC_COUNT {
            q.queue(i)
        }

        join.join().unwrap();
    }

    #[test]
    fn queue_dequeue_many_spmc() {
        fn recv(_t: i32, q: &SegQueue<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.dequeue() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 {
                        break;
                    }
                }
            }
        }

        let q: Arc<SegQueue<i64>> = Arc::new(SegQueue::new());
        let mut v = Vec::new();
        for i in 0..3 {
            let qr = q.clone();
            v.push(thread::spawn(move || recv(i, &qr)));
        }

        v.push(thread::spawn(move || for i in 0..CONC_COUNT {
            q.queue(i);
        }));

        for i in v {
            i.join().unwrap();
        }
    }

    #[test]
    fn queue_dequeue_many_mpmc() {
        enum LR {
            Left(i64),
            Right(i64),
        }

        let q: Arc<SegQueue<LR>> = Arc::new(SegQueue::new());

        let mut v = Vec::new();

        for _t in 0..2 {
            let qc = q.clone();
            v.push(thread::spawn(move || for i in CONC_COUNT - 1..CONC_COUNT {
                qc.queue(LR::Left(i))
            }));
            let qc = q.clone();
            v.push(thread::spawn(move || for i in CONC_COUNT - 1..CONC_COUNT {
                qc.queue(LR::Right(i))
            }));
            let qc = q.clone();
            v.push(thread::spawn(move || {
                let mut vl = vec![];
                let mut vr = vec![];
                for _i in 0..CONC_COUNT {
                    match qc.dequeue() {
                        Some(LR::Left(x)) => vl.push(x),
                        Some(LR::Right(x)) => vr.push(x),
                        _ => {}
                    }
                }

                let mut vl2 = vl.clone();
                let mut vr2 = vr.clone();
                vl2.sort();
                vr2.sort();

                assert_eq!(vl, vl2);
                assert_eq!(vr, vr2);
            }));
        }

        for i in v {
            i.join().unwrap();
        }
    }
}
