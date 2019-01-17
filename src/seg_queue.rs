use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{self, AtomicPtr, AtomicUsize, Ordering};
use std::thread;

use utils::CachePadded;

// Bits indicating the state of a slot:
// * If a value has been written into the slot, `WRITE` is set.
// * If a value has been read from the slot, `READ` is set.
// * If the block is being destroyed, `DESTROY` is set.
const WRITE: usize = 1;
const READ: usize = 2;
const DESTROY: usize = 4;

// Each block covers one "lap" of indices.
const LAP: usize = 32;
// The maximum number of values a block can hold.
const BLOCK_CAP: usize = LAP - 1;
// How many lower bits are reserved for metadata.
const SHIFT: usize = 1;
// Indicates that the block is not the last one.
const HAS_NEXT: usize = 1;

/// A slot in a block.
struct Slot<T> {
    /// The value.
    value: UnsafeCell<ManuallyDrop<T>>,

    /// The state of the slot.
    state: AtomicUsize,
}

impl<T> Slot<T> {
    /// Waits until a value is written into the slot.
    fn wait_write(&self) {
        let mut backoff = Backoff::new();
        while self.state.load(Ordering::Acquire) & WRITE == 0 {
            backoff.snooze();
        }
    }
}

/// A block in a linked list.
///
/// Each block in the list can hold up to `BLOCK_CAP` values.
struct Block<T> {
    /// The next block in the linked list.
    next: AtomicPtr<Block<T>>,

    /// Slots for values.
    slots: [Slot<T>; BLOCK_CAP],
}

impl<T> Block<T> {
    /// Creates an empty block that starts at `start_index`.
    fn new() -> Block<T> {
        unsafe { mem::zeroed() }
    }

    /// Waits until the next pointer is set.
    fn wait_next(&self) -> *mut Block<T> {
        let mut backoff = Backoff::new();
        loop {
            let next = self.next.load(Ordering::Acquire);
            if !next.is_null() {
                return next;
            }
            backoff.snooze();
        }
    }

    /// Sets the `DESTROY` bit in slots starting from `start` and destroys the block.
    unsafe fn destroy(this: *mut Block<T>, start: usize) {
        // It is not necessary to set the `DESTROY` bit in the last slot because that slot has
        // begun destruction of the block.
        for i in start..BLOCK_CAP - 1 {
            let slot = (*this).slots.get_unchecked(i);

            // Mark the `DESTROY` bit if a thread is still using the slot.
            if slot.state.load(Ordering::Acquire) & READ == 0
                && slot.state.fetch_or(DESTROY, Ordering::AcqRel) & READ == 0
            {
                // If a thread is still using the slot, it will continue destruction of the block.
                return;
            }
        }

        // No thread is using the block, now it is safe to destroy it.
        drop(Box::from_raw(this));
    }
}

/// A position in a queue.
#[derive(Debug)]
struct Position<T> {
    /// The index in the queue.
    index: AtomicUsize,

    /// The block in the linked list.
    block: AtomicPtr<Block<T>>,
}

/// Unbounded queue that allocates segments of values.
///
/// This queue can be shared among threads and used with any number of producers or consumers.
#[derive(Debug)]
pub struct SegQueue<T> {
    /// The head of the queue.
    head: CachePadded<Position<T>>,

    /// The tail of the queue.
    tail: CachePadded<Position<T>>,

    /// Indicates that dropping a `SegQueue<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for SegQueue<T> {}
unsafe impl<T: Send> Sync for SegQueue<T> {}

impl<T> SegQueue<T> {
    /// Creates a new unbounded queue.
    pub fn new() -> SegQueue<T> {
        SegQueue {
            head: CachePadded::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            tail: CachePadded::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            _marker: PhantomData,
        }
    }

    /// Pushes a value into the queue.
    pub fn push(&self, value: T) {
        let mut backoff = Backoff::new();
        let mut tail = self.tail.index.load(Ordering::Acquire);
        let mut block = self.tail.block.load(Ordering::Acquire);
        let mut next_block = None;

        loop {
            // Calculate the offset of the index into the block.
            let offset = (tail >> SHIFT) % LAP;

            // If we reached the end of the block, wait until the next one is installed.
            if offset == BLOCK_CAP {
                backoff.snooze();
                tail = self.tail.index.load(Ordering::Acquire);
                block = self.tail.block.load(Ordering::Acquire);
                continue;
            }

            // If we're going to have to install the next block, allocate it in advance in order to
            // make the wait for other threads as short as possible.
            if offset + 1 == BLOCK_CAP && next_block.is_none() {
                next_block = Some(Box::new(Block::<T>::new()));
            }

            // If this is the first push operation, we need to allocate the first block.
            if block.is_null() {
                let new = Box::into_raw(Box::new(Block::<T>::new()));

                if self.tail.block.compare_and_swap(block, new, Ordering::Release) == block {
                    self.head.block.store(new, Ordering::Release);
                    block = new;
                } else {
                    next_block = unsafe { Some(Box::from_raw(new)) };
                    tail = self.tail.index.load(Ordering::Acquire);
                    block = self.tail.block.load(Ordering::Acquire);
                    continue;
                }
            }

            let new_tail = tail + (1 << SHIFT);

            // Try advancing the tail forward.
            match self.tail.index
                .compare_exchange_weak(
                    tail,
                    new_tail,
                    Ordering::SeqCst,
                    Ordering::Acquire,
                )
            {
                Ok(_) => unsafe {
                    // If we've reached the end of the block, install the next one.
                    if offset + 1 == BLOCK_CAP {
                        let next_block = Box::into_raw(next_block.unwrap());
                        let next_index = new_tail.wrapping_add(1 << SHIFT);

                        self.tail.block.store(next_block, Ordering::Release);
                        self.tail.index.store(next_index, Ordering::Release);
                        (*block).next.store(next_block, Ordering::Release);
                    }

                    // Write the value into the slot.
                    let slot = (*block).slots.get_unchecked(offset);
                    slot.value.get().write(ManuallyDrop::new(value));
                    slot.state.fetch_or(WRITE, Ordering::Release);

                    return;
                }
                Err(t) => {
                    tail = t;
                    block = self.tail.block.load(Ordering::Acquire);
                    backoff.spin();
                }
            }
        }
    }

    /// Pops a value from the queue.
    pub fn try_pop(&self) -> Option<T> {
        let mut backoff = Backoff::new();
        let mut head = self.head.index.load(Ordering::Acquire);
        let mut block = self.head.block.load(Ordering::Acquire);

        loop {
            // Calculate the offset of the index into the block.
            let offset = (head >> SHIFT) % LAP;

            // If we reached the end of the block, wait until the next one is installed.
            if offset == BLOCK_CAP {
                backoff.snooze();
                head = self.head.index.load(Ordering::Acquire);
                block = self.head.block.load(Ordering::Acquire);
                continue;
            }

            let mut new_head = head + (1 << SHIFT);

            if new_head & HAS_NEXT == 0 {
                atomic::fence(Ordering::SeqCst);
                let tail = self.tail.index.load(Ordering::Relaxed);

                // If the tail equals the head, that means the queue is empty.
                if head >> SHIFT == tail >> SHIFT {
                    return None;
                }

                // If head and tail are not in the same block, set `HAS_NEXT` in head.
                if (head >> SHIFT) / LAP != (tail >> SHIFT) / LAP {
                    new_head |= HAS_NEXT;
                }
            }

            // The block can be null here only if the first push operation is in progress. In that
            // case, just wait until it gets initialized.
            if block.is_null() {
                backoff.snooze();
                head = self.head.index.load(Ordering::Acquire);
                block = self.head.block.load(Ordering::Acquire);
                continue;
            }

            // Try moving the head index forward.
            match self.head.index
                .compare_exchange_weak(
                    head,
                    new_head,
                    Ordering::SeqCst,
                    Ordering::Acquire,
                )
            {
                Ok(_) => unsafe {
                    // If we've reached the end of the block, move to the next one.
                    if offset + 1 == BLOCK_CAP {
                        let next = (*block).wait_next();
                        let mut next_index = (new_head & !HAS_NEXT).wrapping_add(1 << SHIFT);
                        if !(*next).next.load(Ordering::Relaxed).is_null() {
                            next_index |= HAS_NEXT;
                        }

                        self.head.block.store(next, Ordering::Release);
                        self.head.index.store(next_index, Ordering::Release);
                    }

                    // Read the value.
                    let slot = (*block).slots.get_unchecked(offset);
                    slot.wait_write();
                    let m = slot.value.get().read();
                    let value = ManuallyDrop::into_inner(m);

                    // Destroy the block if we've reached the end, or if another thread wanted to
                    // destroy but couldn't because we were busy reading from the slot.
                    if offset + 1 == BLOCK_CAP {
                        Block::destroy(block, 0);
                    } else if slot.state.fetch_or(READ, Ordering::AcqRel) & DESTROY != 0 {
                        Block::destroy(block, offset + 1);
                    }

                    return Some(value);
                }
                Err(h) => {
                    head = h;
                    block = self.head.block.load(Ordering::Acquire);
                    backoff.spin();
                }
            }
        }
    }

    /// Returns `true` if the queue is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.index.load(Ordering::SeqCst);
        let tail = self.tail.index.load(Ordering::SeqCst);
        head >> SHIFT == tail >> SHIFT
    }

    /// Returns the current number of values in the queue.
    pub fn len(&self) -> usize {
        loop {
            // Load the tail index, then load the head index.
            let mut tail = self.tail.index.load(Ordering::SeqCst);
            let mut head = self.head.index.load(Ordering::SeqCst);

            // If the tail index didn't change, we've got consistent indices to work with.
            if self.tail.index.load(Ordering::SeqCst) == tail {
                // Erase the lower bits.
                tail &= !((1 << SHIFT) - 1);
                head &= !((1 << SHIFT) - 1);

                // Rotate indices so that head falls into the first block.
                let lap = (head >> SHIFT) / LAP;
                tail = tail.wrapping_sub((lap * LAP) << SHIFT);
                head = head.wrapping_sub((lap * LAP) << SHIFT);

                // Remove the lower bits.
                tail >>= SHIFT;
                head >>= SHIFT;

                // Fix up indices if they fall onto block ends.
                if head == BLOCK_CAP {
                    head = 0;
                    tail -= LAP;
                }
                if tail == BLOCK_CAP {
                    tail += 1;
                }

                // Return the difference minus the number of blocks between tail and head.
                return tail - head - tail / LAP;
            }
        }
    }
}

impl<T> Drop for SegQueue<T> {
    fn drop(&mut self) {
        let mut head = self.head.index.load(Ordering::Relaxed);
        let mut tail = self.tail.index.load(Ordering::Relaxed);
        let mut block = self.head.block.load(Ordering::Relaxed);

        // Erase the lower bits.
        head &= !((1 << SHIFT) - 1);
        tail &= !((1 << SHIFT) - 1);

        unsafe {
            // Drop all values between `head` and `tail` and deallocate the heap-allocated blocks.
            while head != tail {
                let offset = (head >> SHIFT) % LAP;

                if offset < BLOCK_CAP {
                    // Drop the value in the slot.
                    let slot = (*block).slots.get_unchecked(offset);
                    ManuallyDrop::drop(&mut *(*slot).value.get());
                } else {
                    // Deallocate the block and move to the next one.
                    let next = (*block).next.load(Ordering::Relaxed);
                    drop(Box::from_raw(block));
                    block = next;
                }

                head = head.wrapping_add(1 << SHIFT);
            }

            // Deallocate the last remaining block.
            if !block.is_null() {
                drop(Box::from_raw(block));
            }
        }
    }
}

impl<T> Default for SegQueue<T> {
    fn default() -> SegQueue<T> {
        SegQueue::new()
    }
}

struct Backoff(u32);

impl Backoff {
    /// Creates a new `Backoff`.
    #[inline]
    pub fn new() -> Backoff {
        Backoff(0)
    }

    /// Backs off in a spin loop.
    ///
    /// This method may yield the current processor. Use it in lock-free retry loops.
    #[inline]
    pub fn spin(&mut self) {
        for _ in 0..1 << self.0.min(6) {
            atomic::spin_loop_hint();
        }
        self.0 = self.0.wrapping_add(1);
    }

    /// Backs off in a wait loop.
    ///
    /// Returns `true` if snoozing has reached a threshold where we should consider parking the
    /// thread instead.
    ///
    /// This method may yield the current processor or the current thread. Use it when waiting on a
    /// resource.
    #[inline]
    pub fn snooze(&mut self) -> bool {
        if self.0 <= 6 {
            for _ in 0..1 << self.0 {
                atomic::spin_loop_hint();
            }
        } else {
            thread::yield_now();
        }

        self.0 = self.0.wrapping_add(1);
        self.0 <= 10
    }
}

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use super::*;
    use scope;

    #[test]
    fn push_pop_1() {
        let q: SegQueue<i64> = SegQueue::new();
        q.push(37);
        assert_eq!(q.try_pop(), Some(37));
    }

    #[test]
    fn push_pop_2() {
        let q: SegQueue<i64> = SegQueue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.try_pop(), Some(37));
        assert_eq!(q.try_pop(), Some(48));
    }

    #[test]
    fn push_pop_empty_check() {
        let q: SegQueue<i64> = SegQueue::new();
        assert_eq!(q.is_empty(), true);
        q.push(42);
        assert_eq!(q.is_empty(), false);
        assert_eq!(q.try_pop(), Some(42));
        assert_eq!(q.is_empty(), true);
    }

    #[test]
    fn push_pop_many_seq() {
        let q: SegQueue<i64> = SegQueue::new();
        for i in 0..200 {
            q.push(i)
        }
        for i in 0..200 {
            assert_eq!(q.try_pop(), Some(i));
        }
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: SegQueue<i64> = SegQueue::new();

        scope(|scope| {
            scope.spawn(|_| {
                let mut next = 0;

                while next < CONC_COUNT {
                    if let Some(elem) = q.try_pop() {
                        assert_eq!(elem, next);
                        next += 1;
                    }
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        }).unwrap();
    }

    #[test]
    fn push_pop_many_spmc() {
        fn recv(_t: i32, q: &SegQueue<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.try_pop() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 {
                        break;
                    }
                }
            }
        }

        let q: SegQueue<i64> = SegQueue::new();
        scope(|scope| {
            for i in 0..3 {
                let q = &q;
                scope.spawn(move |_| recv(i, q));
            }

            scope.spawn(|_| {
                for i in 0..CONC_COUNT {
                    q.push(i);
                }
            });
        }).unwrap();
    }

    #[test]
    fn push_pop_many_mpmc() {
        enum LR {
            Left(i64),
            Right(i64),
        }

        let q: SegQueue<LR> = SegQueue::new();

        scope(|scope| {
            for _t in 0..2 {
                scope.spawn(|_| {
                    for i in CONC_COUNT - 1..CONC_COUNT {
                        q.push(LR::Left(i))
                    }
                });
                scope.spawn(|_| {
                    for i in CONC_COUNT - 1..CONC_COUNT {
                        q.push(LR::Right(i))
                    }
                });
                scope.spawn(|_| {
                    let mut vl = vec![];
                    let mut vr = vec![];
                    for _i in 0..CONC_COUNT {
                        match q.try_pop() {
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
                });
            }
        }).unwrap();
    }
}
