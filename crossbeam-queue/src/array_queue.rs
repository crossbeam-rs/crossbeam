//! The implementation is based on Dmitry Vyukov's bounded MPMC queue.
//!
//! Source:
//!   - <http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue>

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::sync::atomic::{self, AtomicUsize, Ordering};

use crossbeam_utils::{Backoff, CachePadded};

/// A slot in a queue.
struct Slot<T> {
    /// The current stamp.
    ///
    /// If the stamp equals the tail, this node will be next written to. If it equals head + 1,
    /// this node will be next read from.
    stamp: AtomicUsize,

    /// The value in this slot.
    value: UnsafeCell<MaybeUninit<T>>,
}

/// A bounded multi-producer multi-consumer queue.
///
/// This queue allocates a fixed-capacity buffer on construction, which is used to store pushed
/// elements. The queue cannot hold more elements than the buffer allows. Attempting to push an
/// element into a full queue will fail. Alternatively, [`force_push`] makes it possible for
/// this queue to be used as a ring-buffer. Having a buffer allocated upfront makes this queue
/// a bit faster than [`SegQueue`].
///
/// [`force_push`]: ArrayQueue::force_push
/// [`SegQueue`]: super::SegQueue
///
/// # Examples
///
/// ```
/// use crossbeam_queue::ArrayQueue;
///
/// let q = ArrayQueue::new(2);
///
/// assert_eq!(q.push('a'), Ok(()));
/// assert_eq!(q.push('b'), Ok(()));
/// assert_eq!(q.push('c'), Err('c'));
/// assert_eq!(q.pop(), Some('a'));
/// ```
pub struct ArrayQueue<T> {
    /// The head of the queue.
    ///
    /// This value is a "stamp" consisting of an index into the buffer and a lap, but packed into a
    /// single `usize`. The lower bits represent the index, while the upper bits represent the lap.
    ///
    /// Elements are popped from the head of the queue.
    head: CachePadded<AtomicUsize>,

    /// The tail of the queue.
    ///
    /// This value is a "stamp" consisting of an index into the buffer and a lap, but packed into a
    /// single `usize`. The lower bits represent the index, while the upper bits represent the lap.
    ///
    /// Elements are pushed into the tail of the queue.
    tail: CachePadded<AtomicUsize>,

    /// The buffer holding slots.
    buffer: Box<[Slot<T>]>,

    /// The queue capacity.
    cap: usize,

    /// A stamp with the value of `{ lap: 1, index: 0 }`.
    one_lap: usize,
}

unsafe impl<T: Send> Sync for ArrayQueue<T> {}
unsafe impl<T: Send> Send for ArrayQueue<T> {}

impl<T> ArrayQueue<T> {
    /// Creates a new bounded queue with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if the capacity is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::<i32>::new(100);
    /// ```
    pub fn new(cap: usize) -> ArrayQueue<T> {
        let mut new = ArrayQueue {
            buffer: Box::new([]),
            cap: 0,
            one_lap: 1,
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
        };
        new.resize(cap);
        new
    }

    fn index(&self, stamp: usize) -> (usize, usize) {
        // Deconstruct the tail.
        let index = stamp & (self.one_lap - 1);
        let lap = stamp & !(self.one_lap - 1);

        let new = if index + 1 < self.cap {
            // Same lap, incremented index.
            // Set to `{ lap: lap, index: index + 1 }`.
            stamp + 1
        } else {
            // One lap forward, index wraps around to zero.
            // Set to `{ lap: lap.wrapping_add(1), index: 0 }`.
            lap.wrapping_add(self.one_lap)
        };
        (index, new)
    }

    fn push_or_else<F>(&self, mut value: T, f: F) -> Result<(), T>
    where
        F: Fn(T, usize, usize, &Slot<T>) -> Result<T, T>,
    {
        let backoff = Backoff::new();
        let mut tail = self.tail.load(Ordering::Relaxed);

        loop {
            let (index, new_tail) = self.index(tail);

            // Inspect the corresponding slot.
            debug_assert!(index < self.buffer.len());
            let slot = unsafe { self.buffer.get_unchecked(index) };
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the tail and the stamp match, we may attempt to push.
            if tail == stamp {
                // Try moving the tail.
                match self.tail.compare_exchange_weak(
                    tail,
                    new_tail,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // Write the value into the slot and update the stamp.
                        unsafe {
                            slot.value.get().write(MaybeUninit::new(value));
                        }
                        slot.stamp.store(tail + 1, Ordering::Release);
                        return Ok(());
                    }
                    Err(t) => {
                        tail = t;
                        backoff.spin();
                    }
                }
            } else if stamp.wrapping_add(self.one_lap) == tail + 1 {
                atomic::fence(Ordering::SeqCst);
                value = f(value, tail, new_tail, slot)?;
                backoff.spin();
                tail = self.tail.load(Ordering::Relaxed);
            } else {
                // Snooze because we need to wait for the stamp to get updated.
                backoff.snooze();
                tail = self.tail.load(Ordering::Relaxed);
            }
        }
    }

    /// Attempts to push an element into the queue.
    ///
    /// If the queue is full, the element is returned back as an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::new(1);
    ///
    /// assert_eq!(q.push(10), Ok(()));
    /// assert_eq!(q.push(20), Err(20));
    /// ```
    pub fn push(&self, value: T) -> Result<(), T> {
        self.push_or_else(value, |v, tail, _, _| {
            let head = self.head.load(Ordering::Relaxed);

            // If the head lags one lap behind the tail as well...
            if head.wrapping_add(self.one_lap) == tail {
                // ...then the queue is full.
                Err(v)
            } else {
                Ok(v)
            }
        })
    }

    /// Pushes an element into the queue, replacing the oldest element if necessary.
    ///
    /// If the queue is full, the oldest element is replaced and returned,
    /// otherwise `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::new(2);
    ///
    /// assert_eq!(q.force_push(10), None);
    /// assert_eq!(q.force_push(20), None);
    /// assert_eq!(q.force_push(30), Some(10));
    /// assert_eq!(q.pop(), Some(20));
    /// ```
    pub fn force_push(&self, value: T) -> Option<T> {
        self.push_or_else(value, |v, tail, new_tail, slot| {
            let head = tail.wrapping_sub(self.one_lap);
            let new_head = new_tail.wrapping_sub(self.one_lap);

            // Try moving the head.
            if self
                .head
                .compare_exchange_weak(head, new_head, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                // Move the tail.
                self.tail.store(new_tail, Ordering::SeqCst);

                // Swap the previous value.
                let old = unsafe { slot.value.get().replace(MaybeUninit::new(v)).assume_init() };

                // Update the stamp.
                slot.stamp.store(tail + 1, Ordering::Release);

                Err(old)
            } else {
                Ok(v)
            }
        })
        .err()
    }

    /// Attempts to pop an element from the queue.
    ///
    /// If the queue is empty, `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::new(1);
    /// assert_eq!(q.push(10), Ok(()));
    ///
    /// assert_eq!(q.pop(), Some(10));
    /// assert!(q.pop().is_none());
    /// ```
    pub fn pop(&self) -> Option<T> {
        let backoff = Backoff::new();
        let mut head = self.head.load(Ordering::Relaxed);

        loop {
            let (index, new_head) = self.index(head);

            // Inspect the corresponding slot.
            debug_assert!(index < self.buffer.len());
            let slot = unsafe { self.buffer.get_unchecked(index) };
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the the stamp is ahead of the head by 1, we may attempt to pop.
            if head + 1 == stamp {
                // Try moving the head.
                match self.head.compare_exchange_weak(
                    head,
                    new_head,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // Read the value from the slot and update the stamp.
                        let msg = unsafe { slot.value.get().read().assume_init() };
                        slot.stamp
                            .store(head.wrapping_add(self.one_lap), Ordering::Release);
                        return Some(msg);
                    }
                    Err(h) => {
                        head = h;
                        backoff.spin();
                    }
                }
            } else if stamp == head {
                atomic::fence(Ordering::SeqCst);
                let tail = self.tail.load(Ordering::Relaxed);

                // If the tail equals the head, that means the channel is empty.
                if tail == head {
                    return None;
                }

                backoff.spin();
                head = self.head.load(Ordering::Relaxed);
            } else {
                // Snooze because we need to wait for the stamp to get updated.
                backoff.snooze();
                head = self.head.load(Ordering::Relaxed);
            }
        }
    }

    fn try_push_sync(&mut self, value: T) -> Result<(), (T, usize, usize)> {
        let tail = *self.tail.get_mut();
        let (index, new_tail) = self.index(tail);

        // Inspect the corresponding slot.
        debug_assert!(index < self.buffer.len());
        let slot = unsafe { self.buffer.get_unchecked_mut(index) };
        let stamp = *slot.stamp.get_mut();

        // If the tail and the stamp match, we may push.
        if tail == stamp {
            *self.tail.get_mut() = new_tail;
            slot.value.get_mut().write(value);
            *slot.stamp.get_mut() = tail + 1;
            Ok(())
        } else {
            Err((value, new_tail, index))
        }
    }

    /// Attempts to push an element into the queue.
    ///
    /// If the queue is full, the element is returned back as an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let mut q = ArrayQueue::new(1);
    ///
    /// assert_eq!(q.push_sync(10), Ok(()));
    /// assert_eq!(q.push_sync(20), Err(20));
    /// ```
    pub fn push_sync(&mut self, value: T) -> Result<(), T> {
        self.try_push_sync(value).map_err(|(value, ..)| value)
    }

    /// Pushes an element into the queue, replacing the oldest element if necessary.
    ///
    /// If the queue is full, the oldest element is replaced and returned,
    /// otherwise `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let mut q = ArrayQueue::new(2);
    ///
    /// assert_eq!(q.force_push_sync(10), None);
    /// assert_eq!(q.force_push_sync(20), None);
    /// assert_eq!(q.force_push_sync(30), Some(10));
    /// assert_eq!(q.pop_sync(), Some(20));
    /// ```
    pub fn force_push_sync(&mut self, value: T) -> Option<T> {
        match self.try_push_sync(value) {
            Ok(()) => None,
            Err((value, new_tail, index)) => {
                // move the head and tail
                *self.head.get_mut() = new_tail.wrapping_sub(self.one_lap);
                let tail = *self.tail.get_mut();
                *self.tail.get_mut() = new_tail;

                // Inspect the corresponding slot.
                debug_assert!(index < self.buffer.len());
                let slot = unsafe { self.buffer.get_unchecked_mut(index) };

                // Swap the previous value.
                let old =
                    std::mem::replace(unsafe { slot.value.get_mut().assume_init_mut() }, value);

                // Update the stamp.
                *slot.stamp.get_mut() = tail + 1;

                Some(old)
            }
        }
    }

    /// Attempts to pop an element from the queue.
    ///
    /// If the queue is empty, `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let mut q = ArrayQueue::new(1);
    /// assert_eq!(q.push_sync(10), Ok(()));
    ///
    /// assert_eq!(q.pop_sync(), Some(10));
    /// assert!(q.pop_sync().is_none());
    /// ```
    pub fn pop_sync(&mut self) -> Option<T> {
        let head = *self.head.get_mut();
        if head != *self.tail.get_mut() {
            let (index, new_head) = self.index(head);

            // update head
            *self.head.get_mut() = new_head;

            // SAFETY: We have mutable access to this, so we can read without
            // worrying about concurrency. Furthermore, we know this is
            // initialized because it is the value pointed at by `value.head`
            // and this is a non-empty queue.
            debug_assert!(index < self.buffer.len());
            let slot = unsafe { self.buffer.get_unchecked_mut(index) };

            // get value
            let val = unsafe { slot.value.get().read().assume_init() };

            // update stamp
            *slot.stamp.get_mut() = head.wrapping_add(self.one_lap);
            Option::Some(val)
        } else {
            Option::None
        }
    }

    /// Resets the buffer such that all the laps are set to 0 and the head is at index 0
    fn reset(&mut self) {
        let tail = self.tail.get_mut();
        let head = self.head.get_mut();

        let hix = *head & (self.one_lap - 1);
        let tix = *tail & (self.one_lap - 1);

        // rotate head to the very front
        self.buffer.rotate_left(hix);

        // reset all the slot stamps to `{ lap: 0, index: i }`
        for (i, slot) in self.buffer.iter_mut().enumerate() {
            *slot.stamp.get_mut() = i;
        }

        // get the new stamps for the head and tail
        *head = 0;
        *tail = if hix < tix {
            tix - hix
        } else if hix > tix {
            self.cap - hix + tix
        } else if *tail == *head {
            0
        } else {
            self.cap
        };
    }

    /// Resizes the [`ArrayQueue`] to support more entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let mut q = ArrayQueue::new(1);
    /// assert_eq!(q.push_sync(10), Ok(()));
    /// // not enough space
    /// assert_eq!(q.push_sync(20), Err(20));
    ///
    /// q.resize(2);
    /// assert_eq!(q.push_sync(20), Ok(()));
    /// assert_eq!(q.pop_sync(), Some(10));
    ///
    /// q.resize(1);
    /// assert_eq!(q.pop_sync(), Some(20));
    /// ```
    pub fn resize(&mut self, cap: usize) {
        assert!(cap > 0, "capacity must be non-zero");

        self.reset();
        let tail = self.tail.get_mut();
        self.one_lap = (cap + 1).next_power_of_two();
        self.cap = cap;

        // get our buffer as a vec (so we can resize it). Replacing it with an empty slice for now
        let mut v = std::mem::replace(&mut self.buffer, Box::new([])).into_vec();

        // if we want more space, reserve it and initialise
        // else, truncate and shrink
        if cap > v.len() {
            // reserve_exact to optimise the `into_boxed_slice` call later
            v.reserve_exact(cap - v.len());
            for (i, slot) in (v.len()..cap).zip(v.spare_capacity_mut()) {
                slot.write(Slot {
                    // Set the stamp to `{ lap: 0, index: i }`.
                    stamp: AtomicUsize::new(i),
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                });
            }
            // Safety: we have just initialised these values
            unsafe { v.set_len(cap) }
        } else {
            if *tail > cap {
                // drop values
                unsafe {
                    for slot in v.get_unchecked_mut(cap..*tail) {
                        slot.value.get_mut().assume_init_drop();
                    }
                }
                *tail = self.one_lap; // tail wraps around
            }

            v.truncate(cap);
            v.shrink_to_fit();
        }

        // we have used `reserve_exact` and `shrink_to_fit` which guarantee to call
        // the allocator with the `cap` we gave it.
        // This means that we are safe to give that memory to a box, because we can dealloc using that same cap
        let b = unsafe { Box::from_raw(v.leak()) };

        // move our new box back into the buffer, leaking the placeholder
        // (LLVM will probably assume it's not-empty)
        Box::leak(std::mem::replace(&mut self.buffer, b));
    }

    /// Returns the capacity of the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::<i32>::new(100);
    ///
    /// assert_eq!(q.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Returns `true` if the queue is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::new(100);
    ///
    /// assert!(q.is_empty());
    /// q.push(1).unwrap();
    /// assert!(!q.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::SeqCst);
        let tail = self.tail.load(Ordering::SeqCst);

        // Is the tail lagging one lap behind head?
        // Is the tail equal to the head?
        //
        // Note: If the head changes just before we load the tail, that means there was a moment
        // when the channel was not empty, so it is safe to just return `false`.
        tail == head
    }

    /// Returns `true` if the queue is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::new(1);
    ///
    /// assert!(!q.is_full());
    /// q.push(1).unwrap();
    /// assert!(q.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        let tail = self.tail.load(Ordering::SeqCst);
        let head = self.head.load(Ordering::SeqCst);

        // Is the head lagging one lap behind tail?
        //
        // Note: If the tail changes just before we load the head, that means there was a moment
        // when the queue was not full, so it is safe to just return `false`.
        head.wrapping_add(self.one_lap) == tail
    }

    /// Returns the number of elements in the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::ArrayQueue;
    ///
    /// let q = ArrayQueue::new(100);
    /// assert_eq!(q.len(), 0);
    ///
    /// q.push(10).unwrap();
    /// assert_eq!(q.len(), 1);
    ///
    /// q.push(20).unwrap();
    /// assert_eq!(q.len(), 2);
    /// ```
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
                } else if tail == head {
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
        let head = *self.head.get_mut();
        let tail = *self.tail.get_mut();

        let hix = head & (self.one_lap - 1);
        let tix = tail & (self.one_lap - 1);

        let len = if hix < tix {
            tix - hix
        } else if hix > tix {
            self.cap - hix + tix
        } else if tail == head {
            0
        } else {
            self.cap
        };

        // Loop over all slots that hold a message and drop them.
        for i in 0..len {
            // Compute the index of the next slot holding a message.
            let index = if hix + i < self.cap {
                hix + i
            } else {
                hix + i - self.cap
            };

            unsafe {
                debug_assert!(index < self.buffer.len());
                let slot = self.buffer.get_unchecked_mut(index);
                let value = &mut *slot.value.get();
                value.as_mut_ptr().drop_in_place();
            }
        }
    }
}

impl<T> fmt::Debug for ArrayQueue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("ArrayQueue { .. }")
    }
}

impl<T> IntoIterator for ArrayQueue<T> {
    type Item = T;

    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { value: self }
    }
}

#[derive(Debug)]
pub struct IntoIter<T> {
    value: ArrayQueue<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.value.pop_sync()
    }
}
