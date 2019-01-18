/// A bounded multi-producer multi-consumer queue.
///
/// The implementation is based on Dmitry Vyukov's bounded MPMC queue:
///
/// * http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue
/// * https://docs.google.com/document/d/1yIAYmbvL3JxOKOjuCyon7JhW4cSv1wy5hC0ApeGMV9s/pub

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use queue::{PopError, PushError};
use utils::CachePadded;

/// A slot in a queue.
struct Slot<T> {
    /// The current stamp.
    ///
    /// If the stamp equals the tail, this node will be next written to. If it equals the head,
    /// this node will be next read from.
    stamp: AtomicUsize,

    /// The element in this slot.
    ///
    /// If the lap in the stamp is odd, this value contains a element. Otherwise, it is empty.
    value: UnsafeCell<T>,
}

/// A bounded multi-producer multi-consumer queue.
pub struct ArrayQueue<T> {
    /// The head of the queue.
    ///
    /// This value is a "stamp" consisting of an index into the buffer and a lap, but packed into a
    /// single `usize`. The lower bits represent the index, while the upper bits represent the lap.
    /// The lap in the head is always an odd number.
    ///
    /// Elements are popped from the head of the queue.
    head: CachePadded<AtomicUsize>,

    /// The tail of the queue.
    ///
    /// This value is a "stamp" consisting of an index into the buffer and a lap, but packed into a
    /// single `usize`. The lower bits represent the index, while the upper bits represent the lap.
    /// The lap in the tail is always an even number.
    ///
    /// Elements are pushed into the tail of the queue.
    tail: CachePadded<AtomicUsize>,

    /// The buffer holding slots.
    buffer: *mut Slot<T>,

    /// The queue capacity.
    cap: usize,

    /// A stamp with the value of `{ lap: 1, index: 0 }`.
    one_lap: usize,

    /// Indicates that dropping an `ArrayQueue<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Sync for ArrayQueue<T> {}
unsafe impl<T: Send> Send for ArrayQueue<T> {}

impl<T> ArrayQueue<T> {
    /// Creates a new queue of capacity `cap`.
    pub fn new(cap: usize) -> ArrayQueue<T> {
        // One lap is the smallest power of two greater than or equal to `cap`.
        let one_lap = cap.next_power_of_two();

        // Head is initialized to `{ lap: 1, index: 0 }`.
        // Tail is initialized to `{ lap: 0, index: 0 }`.
        let head = one_lap;
        let tail = 0;

        // Allocate a buffer of `cap` slots.
        let buffer = {
            let mut v = Vec::<Slot<T>>::with_capacity(cap);
            let ptr = v.as_mut_ptr();
            mem::forget(v);
            ptr
        };

        // Initialize stamps in the slots.
        for i in 0..cap {
            unsafe {
                // Set the stamp to `{ lap: 0, index: i }`.
                let slot = buffer.add(i);
                ptr::write(&mut (*slot).stamp, AtomicUsize::new(i));
            }
        }

        ArrayQueue {
            buffer,
            cap,
            one_lap,
            head: CachePadded::new(AtomicUsize::new(head)),
            tail: CachePadded::new(AtomicUsize::new(tail)),
            _marker: PhantomData,
        }
    }

    /// Attempts to push `value` into the queue.
    pub fn push(&self, value: T) -> Result<(), PushError<T>> {
        loop {
            // Load the tail and deconstruct it.
            let tail = self.tail.load(Ordering::SeqCst);
            let index = tail & (self.one_lap - 1);
            let lap = tail & !(self.one_lap - 1);

            // Inspect the corresponding slot.
            let slot = unsafe { &*self.buffer.add(index) };
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the tail and the stamp match, we may attempt to push.
            if tail == stamp {
                let new_tail = if index + 1 < self.cap {
                    // Same lap, incremented index.
                    // Set to `{ lap: lap, index: index + 1 }`.
                    tail + 1
                } else {
                    // Two laps forward, index wraps around to zero.
                    // Set to `{ lap: lap.wrapping_add(2), index: 0 }`.
                    lap.wrapping_add(self.one_lap.wrapping_mul(2))
                };

                // Try moving the tail.
                if self
                    .tail
                    .compare_exchange_weak(tail, new_tail, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    // Write the value into the slot and update the stamp.
                    unsafe {
                        slot.value.get().write(value);
                    }
                    slot.stamp.store(stamp.wrapping_add(self.one_lap), Ordering::Release);
                    return Ok(());
                }
            // But if the slot lags one lap behind the tail...
            } else if stamp.wrapping_add(self.one_lap) == tail {
                let head = self.head.load(Ordering::SeqCst);

                // ...and if the head lags one lap behind the tail as well...
                if head.wrapping_add(self.one_lap) == tail {
                    // ...then the queue is full.
                    return Err(PushError(value));
                }
            }
        }
    }

    /// Attempts to pop a value from the queue.
    pub fn pop(&self) -> Result<T, PopError> {
        loop {
            // Load the head and deconstruct it.
            let head = self.head.load(Ordering::SeqCst);
            let index = head & (self.one_lap - 1);
            let lap = head & !(self.one_lap - 1);

            // Inspect the corresponding slot.
            let slot = unsafe { &*self.buffer.add(index) };
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the the head and the stamp match, we may attempt to pop.
            if head == stamp {
                let new = if index + 1 < self.cap {
                    // Same lap, incremented index.
                    // Set to `{ lap: lap, index: index + 1 }`.
                    head + 1
                } else {
                    // Two laps forward, index wraps around to zero.
                    // Set to `{ lap: lap.wrapping_add(2), index: 0 }`.
                    lap.wrapping_add(self.one_lap.wrapping_mul(2))
                };

                // Try moving the head.
                if self
                    .head
                    .compare_exchange_weak(head, new, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    // Read the value from the slot and update the stamp.
                    let msg = unsafe { slot.value.get().read() };
                    slot.stamp.store(stamp.wrapping_add(self.one_lap), Ordering::Release);
                    return Ok(msg);
                }
            // But if the slot lags one lap behind the head...
            } else if stamp.wrapping_add(self.one_lap) == head {
                let tail = self.tail.load(Ordering::SeqCst);

                // ...and if the tail lags one lap behind the head as well, that means the queue
                // is empty.
                if tail.wrapping_add(self.one_lap) == head {
                    return Err(PopError);
                }
            }
        }
    }

    /// Returns the capacity of the queue.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Returns `true` if the queue is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::SeqCst);
        let tail = self.tail.load(Ordering::SeqCst);

        // Is the tail lagging one lap behind head?
        //
        // Note: If the head changes just before we load the tail, that means there was a moment
        // when the queue was not empty, so it is safe to just return `false`.
        tail.wrapping_add(self.one_lap) == head
    }

    /// Returns `true` if the queue is full.
    pub fn is_full(&self) -> bool {
        let tail = self.tail.load(Ordering::SeqCst);
        let head = self.head.load(Ordering::SeqCst);

        // Is the head lagging one lap behind tail?
        //
        // Note: If the tail changes just before we load the head, that means there was a moment
        // when the queue was not full, so it is safe to just return `false`.
        head.wrapping_add(self.one_lap) == tail
    }

    /// Returns the current number of elements inside the queue.
    pub fn len(&self) -> usize {
        loop {
            // Load the tail, then load the head.
            let tail = self.tail.load(Ordering::SeqCst);
            let head = self.head.load(Ordering::SeqCst);

            // If the tail didn't change, we've got consistent values to work with.
            if self.tail.load(Ordering::SeqCst) == tail {
                let hix = head & (self.one_lap - 1);
                let tix = tail & (self.one_lap - 1);

                return if hix < tix {
                    tix - hix
                } else if hix > tix {
                    self.cap - hix + tix
                } else if tail.wrapping_add(self.one_lap) == head {
                    0
                } else {
                    self.cap
                };
            }
        }
    }
}

impl<T> Drop for ArrayQueue<T> {
    fn drop(&mut self) {
        // Get the index of the head.
        let hix = self.head.load(Ordering::Relaxed) & (self.one_lap - 1);

        // Loop over all slots that hold a message and drop them.
        for i in 0..self.len() {
            // Compute the index of the next slot holding a message.
            let index = if hix + i < self.cap {
                hix + i
            } else {
                hix + i - self.cap
            };

            unsafe {
                self.buffer.add(index).drop_in_place();
            }
        }

        // Finally, deallocate the buffer, but don't run any destructors.
        unsafe {
            Vec::from_raw_parts(self.buffer, 0, self.cap);
        }
    }
}

impl<T> fmt::Debug for ArrayQueue<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ArrayQueue {{ ... }}")
    }
}
