use core::mem::{self, ManuallyDrop};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crossbeam_utils::CachePadded;
use {unprotected, Atomic, Guard, Owned, Shared};
use core::marker::PhantomData;

pub struct Queue<T> {
    /// The head of the channel.
    head: CachePadded<Position<T>>,

    /// The tail of the channel.
    tail: CachePadded<Position<T>>,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

// Any particular `T` should never be accessed concurrently, so no need for `Sync`.
unsafe impl<T: Send> Sync for Queue<T> {}
unsafe impl<T: Send> Send for Queue<T> {}

const BLOCK_CAP: usize = 32;
/// A slot in a block.
struct Slot<T> {
    /// The message.
    msg: UnsafeCell<ManuallyDrop<T>>,

    /// Equals `true` if the message is ready for reading.
    ready: AtomicBool,
}

struct Block<T> {
    start_index: usize,

    /// The next block in the linked list.
    next: Atomic<Block<T>>,

    /// Slots for messages.
    slots: [UnsafeCell<Slot<T>>; BLOCK_CAP],
}

struct Position<T> {
    index: AtomicUsize,
    block: Atomic<Block<T>>,
}

impl<T> Block<T> {
    /// Creates an empty block that starts at `start_index`.
    fn new(start_index: usize) -> Block<T> {
        Block {
            start_index,
            slots: unsafe { mem::zeroed() },
            next: Atomic::null(),
        }
    }
}

impl<T> Queue<T> {
    /// Create a new, empty queue.
    pub fn new() -> Queue<T> {
        let queue = Queue {
            head: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                block: Atomic::null(),
            }),
            tail: CachePadded::new(Position {
                index: AtomicUsize::new(0),
                block: Atomic::null(),
            }),
            _marker: PhantomData,
        };

        // Allocate an empty block for the first batch of messages.
        let block = unsafe { Owned::new(Block::new(0)).into_shared(unprotected()) };
        queue.head.block.store(block, Ordering::Relaxed);
        queue.tail.block.store(block, Ordering::Relaxed);

        queue
    }
    
    pub fn push(&self, bag: T, guard: &Guard) {
        loop {
            let tail_ptr = self.tail.block.load(Ordering::Acquire, &guard);
            let tail = unsafe { tail_ptr.deref() };
            let tail_index = self.tail.index.load(Ordering::Relaxed);

            // Calculate the index of the corresponding slot in the block.
            let offset = tail_index.wrapping_sub(tail.start_index);

            // Advance the current index one slot forward.
            let new_index = tail_index.wrapping_add(1);

            // A closure that installs a block following `tail` in case it hasn't been yet.
            let install_next_block = || {
                let current = tail
                    .next
                    .compare_and_set(
                        Shared::null(),
                        Owned::new(Block::new(tail.start_index.wrapping_add(BLOCK_CAP))),
                        Ordering::AcqRel,
                        &guard,
                    ).unwrap_or_else(|err| err.current);

                let _ =
                    self.tail
                        .block
                        .compare_and_set(tail_ptr, current, Ordering::Release, &guard);
            };

            // If `tail_index` is pointing into `tail`...
            if offset < BLOCK_CAP {
                // Try moving the tail index forward.
                if self
                    .tail
                    .index
                    .compare_exchange_weak(
                        tail_index,
                        new_index,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_ok()
                {
                    if offset + 1 == BLOCK_CAP {
                        install_next_block();
                    }

                    unsafe {
                        let slot = tail.slots.get_unchecked(offset).get();
                        (*slot).msg.get().write(ManuallyDrop::new(bag));
                        (*slot).ready.store(true, Ordering::Release);
                    }
                    break;
                }
            } else if offset == BLOCK_CAP {
                install_next_block();
            }
        }
    }

    pub fn try_pop_if<F>(&self, condition: F, guard: &Guard) -> Option<T>
    where
        T: Sync,
        F: Fn(&T) -> bool,
    {
        loop {
            let head_ptr = self.head.block.load(Ordering::Acquire, &guard);
            let head = unsafe { head_ptr.deref() };
            let head_index = self.head.index.load(Ordering::SeqCst);

            let offset = head_index.wrapping_sub(head.start_index);

            let new_index = head_index.wrapping_add(1);

            let install_next_block = || {
                let current = head
                    .next
                    .compare_and_set(
                        Shared::null(),
                        Owned::new(Block::new(head.start_index.wrapping_add(BLOCK_CAP))),
                        Ordering::AcqRel,
                        &guard,
                    ).unwrap_or_else(|err| err.current);

                let _ =
                    self.head
                        .block
                        .compare_and_set(head_ptr, current, Ordering::Release, &guard);

                if self.tail.block.load(Ordering::Acquire, &guard) == head_ptr {
                    let _ =
                        self.tail
                            .block
                            .compare_and_set(head_ptr, current, Ordering::Release, &guard);
                }
            };

            if offset < BLOCK_CAP {
                let slot = unsafe { &*head.slots.get_unchecked(offset).get() };

                if !slot.ready.load(Ordering::Acquire) {
                    return None;
                }
                
                let bag = unsafe{ &slot.msg.get().read() };
                if !condition(bag) {
                    return None;
                }
                
                if self
                    .head
                    .index
                    .compare_exchange_weak(
                        head_index,
                        new_index,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_ok()
                {                    
                    if offset + 1 == BLOCK_CAP {
                        install_next_block();
                        unsafe {
                            guard.defer_destroy(head_ptr);
                        }
                    }
                    
                    unsafe {
                        let data = ManuallyDrop::into_inner(slot.msg.get().read());
                        return Some(data);
                    }
                }
            } else if offset == BLOCK_CAP {
                install_next_block();
            }
        }
    }
}

impl<T> Drop for Queue<T> {
    fn drop(&mut self) {

    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crossbeam_utils::thread;
    use pin;

    struct Queue<T> {
        queue: super::Queue<T>,
    }

    impl<T> Queue<T> {
        pub fn new() -> Queue<T> {
            Queue {
                queue: super::Queue::new(),
            }
        }

        pub fn push(&self, t: T) {
            let guard = &pin();
            self.queue.push(t, guard);
        }

        pub fn is_empty(&self) -> bool {
            let guard = &pin();
            let head = self.queue.head.index.load(Ordering::SeqCst);
            let tail = self.queue.tail.index.load(Ordering::SeqCst);
            head >= tail
        }
    }
}
