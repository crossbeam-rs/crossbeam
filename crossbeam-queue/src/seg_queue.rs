use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::fmt;
use core::marker::PhantomData;
use core::mem::{self, ManuallyDrop, MaybeUninit};
use core::ptr;
use std::sync::atomic::{self, AtomicPtr, AtomicUsize, Ordering};

use crossbeam_utils::{Backoff, CachePadded};

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
// The maximum number of blocks to retain in the block cache. Must be a power of two.
const BLOCK_CACHE_SIZE: usize = 4;
// How many lower bits are reserved for metadata.
const SHIFT: usize = 1;
// Indicates that the block is not the last one.
const HAS_NEXT: usize = 1;

/// A slot in a block.
struct Slot<T> {
    /// The value.
    value: UnsafeCell<MaybeUninit<T>>,

    /// The state of the slot.
    state: AtomicUsize,
}

impl<T> Slot<T> {
    /// Waits until a value is written into the slot.
    fn wait_write(&self) {
        let backoff = Backoff::new();
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
        // SAFETY: This is safe because:
        //  [1] `Block::next` (AtomicPtr) may be safely zero initialized.
        //  [2] `Block::slots` (Array) may be safely zero initialized because of [3, 4].
        //  [3] `Slot::value` (UnsafeCell) may be safely zero initialized because it
        //       holds a MaybeUninit.
        //  [4] `Slot::state` (AtomicUsize) may be safely zero initialized.
        unsafe { MaybeUninit::zeroed().assume_init() }
    }

    /// Waits until the next pointer is set.
    fn wait_next(&self) -> *mut Block<T> {
        let backoff = Backoff::new();
        loop {
            let next = self.next.load(Ordering::Acquire);
            if !next.is_null() {
                return next;
            }
            backoff.snooze();
        }
    }
}

#[cfg(target_pointer_width = "16")]
type Uhalf = u8;
#[cfg(target_pointer_width = "16")]
type AtomicUhalf = std::sync::atomic::AtomicU8;

#[cfg(target_pointer_width = "32")]
type Uhalf = u16;
#[cfg(target_pointer_width = "32")]
type AtomicUhalf = std::sync::atomic::AtomicU16;

#[cfg(target_pointer_width = "64")]
type Uhalf = u32;
#[cfg(target_pointer_width = "64")]
type AtomicUhalf = std::sync::atomic::AtomicU32;

#[repr(C)]
#[cfg(target_endian = "little")]
struct BlockCacheSplitIndices {
    head: AtomicUhalf,
    tail: AtomicUhalf,
}

#[repr(C)]
#[cfg(target_endian = "big")]
struct BlockCacheSplitIndices {
    tail: AtomicUhalf,
    head: AtomicUhalf,
}

#[repr(C)]
union BlockCacheIndices {
    both: ManuallyDrop<AtomicUsize>,
    split: ManuallyDrop<BlockCacheSplitIndices>,
}

struct BlockCache<T> {
    indices: BlockCacheIndices,
    blocks: [AtomicPtr<Block<T>>; BLOCK_CACHE_SIZE],
}

impl<T> BlockCache<T> {
    const fn new() -> Self {
        // As far as I know, we are forced to hard-code the initialization of `blocks` here as
        // `MaybeUninit::zeroed().assume_init()` cannot be used in const functions.
        Self {
            indices: BlockCacheIndices {
                both: ManuallyDrop::new(AtomicUsize::new(0)),
            },
            blocks: [
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
                AtomicPtr::new(ptr::null_mut()),
            ],
        }
    }

    unsafe fn try_get(&self) -> *mut Block<T> {
        loop {
            let both = self.indices.both.load(Ordering::Relaxed);
            let head = both as Uhalf;
            let tail = (both >> (mem::size_of::<Uhalf>() * 8)) as Uhalf;

            if head == tail {
                return ptr::null_mut();
            }

            if self
                .indices
                .split
                .head
                .compare_exchange_weak(head, head + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return self.blocks[head as usize & (BLOCK_CACHE_SIZE - 1)]
                    .swap(ptr::null_mut(), Ordering::Acquire);
            }
        }
    }

    unsafe fn try_put(&self, block: *mut Block<T>) -> *mut Block<T> {
        let both = self.indices.both.load(Ordering::Relaxed);
        let head = both as Uhalf;
        let tail = (both >> (mem::size_of::<Uhalf>() * 8)) as Uhalf;

        if tail - head == BLOCK_CACHE_SIZE as Uhalf {
            return block;
        }

        *block = MaybeUninit::zeroed().assume_init();
        let prev =
            self.blocks[tail as usize & (BLOCK_CACHE_SIZE - 1)].swap(block, Ordering::Release);
        let _ = self.indices.split.tail.compare_exchange_weak(
            tail,
            tail + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        prev
    }
}

/// A position in a queue.
struct Position<T> {
    /// The index in the queue.
    index: AtomicUsize,

    /// The block in the linked list.
    block: AtomicPtr<Block<T>>,
}

/// An unbounded multi-producer multi-consumer queue.
///
/// This queue is implemented as a linked list of segments, where each segment is a small buffer
/// that can hold a handful of elements. There is no limit to how many elements can be in the queue
/// at a time. However, since segments need to be dynamically allocated as elements get pushed,
/// this queue is somewhat slower than [`ArrayQueue`].
///
/// [`ArrayQueue`]: super::ArrayQueue
///
/// # Examples
///
/// ```
/// use crossbeam_queue::SegQueue;
///
/// let q = SegQueue::new();
///
/// q.push('a');
/// q.push('b');
///
/// assert_eq!(q.pop(), Some('a'));
/// assert_eq!(q.pop(), Some('b'));
/// assert!(q.pop().is_none());
/// ```
pub struct SegQueue<T> {
    /// The head of the queue.
    head: CachePadded<Position<T>>,

    /// The tail of the queue.
    tail: CachePadded<Position<T>>,

    /// Cache some number of blocks to avoid allocations when possible.
    block_cache: CachePadded<BlockCache<T>>,

    /// Indicates that dropping a `SegQueue<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for SegQueue<T> {}
unsafe impl<T: Send> Sync for SegQueue<T> {}

impl<T> SegQueue<T> {
    /// Creates a new unbounded queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::SegQueue;
    ///
    /// let q = SegQueue::<i32>::new();
    /// ```
    pub const fn new() -> SegQueue<T> {
        SegQueue {
            head: CachePadded::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            tail: CachePadded::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            block_cache: CachePadded::new(BlockCache::new()),
            _marker: PhantomData,
        }
    }

    /// Pushes an element into the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::SegQueue;
    ///
    /// let q = SegQueue::new();
    ///
    /// q.push(10);
    /// q.push(20);
    /// ```
    pub fn push(&self, value: T) {
        let backoff = Backoff::new();
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

            // If we're going to have to install the next block, get or allocate
            // it in advance in order to make the wait for other threads as
            // short as possible.
            if offset + 1 == BLOCK_CAP && next_block.is_none() {
                let p = unsafe { self.block_cache.try_get() };
                next_block = if p.is_null() {
                    Some(Box::new(Block::new()))
                } else {
                    Some(unsafe { Box::from_raw(p) })
                };
            }

            // If this is the first push operation, we need to allocate the first block.
            if block.is_null() {
                let new = Box::into_raw(Box::new(Block::new()));

                if self
                    .tail
                    .block
                    .compare_exchange(block, new, Ordering::Release, Ordering::Relaxed)
                    .is_ok()
                {
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
            match self.tail.index.compare_exchange_weak(
                tail,
                new_tail,
                Ordering::SeqCst,
                Ordering::Acquire,
            ) {
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
                    slot.value.get().write(MaybeUninit::new(value));
                    slot.state.fetch_or(WRITE, Ordering::Release);

                    return;
                },
                Err(t) => {
                    tail = t;
                    block = self.tail.block.load(Ordering::Acquire);
                    backoff.spin();
                }
            }
        }
    }

    /// Sets the `DESTROY` bit in slots starting from `start` and caches or
    /// destroys the block.
    unsafe fn cache_or_destroy(&self, this: *mut Block<T>, start: usize) {
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

        // No thread is using the block. Try to cache it for reuse or deallocate
        // it otherwise.
        let p = self.block_cache.try_put(this);
        if !p.is_null() {
            drop(Box::from_raw(p));
        }
    }

    /// Pops an element from the queue.
    ///
    /// If the queue is empty, `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::SegQueue;
    ///
    /// let q = SegQueue::new();
    ///
    /// q.push(10);
    /// assert_eq!(q.pop(), Some(10));
    /// assert!(q.pop().is_none());
    /// ```
    pub fn pop(&self) -> Option<T> {
        let backoff = Backoff::new();
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
            match self.head.index.compare_exchange_weak(
                head,
                new_head,
                Ordering::SeqCst,
                Ordering::Acquire,
            ) {
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
                    let value = slot.value.get().read().assume_init();

                    // Destroy the block if we've reached the end, or if another thread wanted to
                    // destroy but couldn't because we were busy reading from the slot.
                    if offset + 1 == BLOCK_CAP {
                        self.cache_or_destroy(block, 0);
                    } else if slot.state.fetch_or(READ, Ordering::AcqRel) & DESTROY != 0 {
                        self.cache_or_destroy(block, offset + 1);
                    }

                    return Some(value);
                },
                Err(h) => {
                    head = h;
                    block = self.head.block.load(Ordering::Acquire);
                    backoff.spin();
                }
            }
        }
    }

    /// Returns `true` if the queue is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::SegQueue;
    ///
    /// let q = SegQueue::new();
    ///
    /// assert!(q.is_empty());
    /// q.push(1);
    /// assert!(!q.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        let head = self.head.index.load(Ordering::SeqCst);
        let tail = self.tail.index.load(Ordering::SeqCst);
        head >> SHIFT == tail >> SHIFT
    }

    /// Returns the number of elements in the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::SegQueue;
    ///
    /// let q = SegQueue::new();
    /// assert_eq!(q.len(), 0);
    ///
    /// q.push(10);
    /// assert_eq!(q.len(), 1);
    ///
    /// q.push(20);
    /// assert_eq!(q.len(), 2);
    /// ```
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

                // Fix up indices if they fall onto block ends.
                if (tail >> SHIFT) & (LAP - 1) == LAP - 1 {
                    tail = tail.wrapping_add(1 << SHIFT);
                }
                if (head >> SHIFT) & (LAP - 1) == LAP - 1 {
                    head = head.wrapping_add(1 << SHIFT);
                }

                // Rotate indices so that head falls into the first block.
                let lap = (head >> SHIFT) / LAP;
                tail = tail.wrapping_sub((lap * LAP) << SHIFT);
                head = head.wrapping_sub((lap * LAP) << SHIFT);

                // Remove the lower bits.
                tail >>= SHIFT;
                head >>= SHIFT;

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
                    let p = &mut *slot.value.get();
                    p.as_mut_ptr().drop_in_place();
                } else {
                    // Deallocate the block and move to the next one.
                    let next = (*block).next.load(Ordering::Relaxed);
                    drop(Box::from_raw(block));
                    block = next;
                }

                head = head.wrapping_add(1 << SHIFT);
            }

            // Deallocate the last remaining block and any cached blocks.
            if !block.is_null() {
                drop(Box::from_raw(block));
            }
            for b in &self.block_cache.blocks {
                let p = b.load(Ordering::Relaxed);
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
            }
        }
    }
}

impl<T> fmt::Debug for SegQueue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("SegQueue { .. }")
    }
}

impl<T> Default for SegQueue<T> {
    fn default() -> SegQueue<T> {
        SegQueue::new()
    }
}
