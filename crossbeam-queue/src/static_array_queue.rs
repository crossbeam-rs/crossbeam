//! The implementation is based on Dmitry Vyukov's bounded MPMC queue.
//!
//! Source:
//!   - <http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue>

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
/// Contrary to [`ArrayQueue`], this queue does not require the `alloc` feature and its
/// constructor is `const`, which makes it ideal for usage as a `static` variable.
///
/// This queue contains a fixed-size array, which is used to store pushed elements.
/// The queue cannot hold more elements than the array allows. Attempting to push an
/// element into a full queue will fail. Alternatively, [`force_push`] makes it possible for
/// this queue to be used as a ring-buffer.
///
/// [`force_push`]: StaticArrayQueue::force_push
/// [`ArrayQueue`]: super::ArrayQueue
///
/// # Examples
///
/// ```
/// use crossbeam_queue::StaticArrayQueue;
///
/// let q = StaticArrayQueue::<char, 2>::new();
///
/// assert_eq!(q.push('a'), Ok(()));
/// assert_eq!(q.push('b'), Ok(()));
/// assert_eq!(q.push('c'), Err('c'));
/// assert_eq!(q.pop(), Some('a'));
/// ```
pub struct StaticArrayQueue<T, const N: usize> {
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
    buffer: [Slot<T>; N],

    /// The queue capacity.
    cap: usize,

    /// A stamp with the value of `{ lap: 1, index: 0 }`.
    one_lap: usize,
}

unsafe impl<T: Send, const N: usize> Sync for StaticArrayQueue<T, N> {}
unsafe impl<T: Send, const N: usize> Send for StaticArrayQueue<T, N> {}

impl<T, const N: usize> StaticArrayQueue<T, N> {
    /// Creates a new bounded queue with the given capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<i32, 100>::new();
    /// ```
    pub const fn new() -> StaticArrayQueue<T, N> {
        assert!(N > 0, "capacity must be non-zero");

        // Head is initialized to `{ lap: 0, index: 0 }`.
        // Tail is initialized to `{ lap: 0, index: 0 }`.
        let head = 0;
        let tail = 0;

        let mut buffer: [MaybeUninit<Slot<T>>; N] = unsafe { MaybeUninit::uninit().assume_init() };

        {
            // const for's are not stabilized yet, so use a loop
            let mut i = 0;
            while i < N {
                // Set the stamp to `{ lap: 0, index: i }`.
                buffer[i] = MaybeUninit::new(Slot {
                    stamp: AtomicUsize::new(i),
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                });

                i += 1;
            }
        }

        // Allocate a buffer of `cap` slots initialized
        // with stamps.
        let buffer: [Slot<T>; N] = unsafe { MaybeUninit::array_assume_init(buffer) };

        // One lap is the smallest power of two greater than `cap`.
        let one_lap = (N + 1).next_power_of_two();

        StaticArrayQueue {
            buffer,
            cap: N,
            one_lap,
            head: CachePadded::new(AtomicUsize::new(head)),
            tail: CachePadded::new(AtomicUsize::new(tail)),
        }
    }

    fn push_or_else<F>(&self, mut value: T, f: F) -> Result<(), T>
    where
        F: Fn(T, usize, usize, &Slot<T>) -> Result<T, T>,
    {
        let backoff = Backoff::new();
        let mut tail = self.tail.load(Ordering::Relaxed);

        loop {
            // Deconstruct the tail.
            let index = tail & (self.one_lap - 1);
            let lap = tail & !(self.one_lap - 1);

            let new_tail = if index + 1 < self.cap {
                // Same lap, incremented index.
                // Set to `{ lap: lap, index: index + 1 }`.
                tail + 1
            } else {
                // One lap forward, index wraps around to zero.
                // Set to `{ lap: lap.wrapping_add(1), index: 0 }`.
                lap.wrapping_add(self.one_lap)
            };

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
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<_, 1>::new();
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
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<_, 2>::new();
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
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<_, 1>::new();
    /// assert_eq!(q.push(10), Ok(()));
    ///
    /// assert_eq!(q.pop(), Some(10));
    /// assert!(q.pop().is_none());
    /// ```
    pub fn pop(&self) -> Option<T> {
        let backoff = Backoff::new();
        let mut head = self.head.load(Ordering::Relaxed);

        loop {
            // Deconstruct the head.
            let index = head & (self.one_lap - 1);
            let lap = head & !(self.one_lap - 1);

            // Inspect the corresponding slot.
            debug_assert!(index < self.buffer.len());
            let slot = unsafe { self.buffer.get_unchecked(index) };
            let stamp = slot.stamp.load(Ordering::Acquire);

            // If the the stamp is ahead of the head by 1, we may attempt to pop.
            if head + 1 == stamp {
                let new = if index + 1 < self.cap {
                    // Same lap, incremented index.
                    // Set to `{ lap: lap, index: index + 1 }`.
                    head + 1
                } else {
                    // One lap forward, index wraps around to zero.
                    // Set to `{ lap: lap.wrapping_add(1), index: 0 }`.
                    lap.wrapping_add(self.one_lap)
                };

                // Try moving the head.
                match self.head.compare_exchange_weak(
                    head,
                    new,
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

    /// Returns the capacity of the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<i32, 100>::new();
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
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<_, 100>::new();
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
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<_, 1>::new();
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
    /// use crossbeam_queue::StaticArrayQueue;
    ///
    /// let q = StaticArrayQueue::<_, 100>::new();
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

impl<T, const N: usize> Drop for StaticArrayQueue<T, N> {
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

impl<T, const N: usize> fmt::Debug for StaticArrayQueue<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("StaticArrayQueue { .. }")
    }
}

impl<T, const N: usize> IntoIterator for StaticArrayQueue<T, N> {
    type Item = T;

    type IntoIter = IntoIter<T, N>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { value: self }
    }
}

#[derive(Debug)]
pub struct IntoIter<T, const N: usize> {
    value: StaticArrayQueue<T, N>,
}

impl<T, const N: usize> Iterator for IntoIter<T, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        let value = &mut self.value;
        let head = *value.head.get_mut();
        if value.head.get_mut() != value.tail.get_mut() {
            let index = head & (value.one_lap - 1);
            let lap = head & !(value.one_lap - 1);
            // SAFETY: We have mutable access to this, so we can read without
            // worrying about concurrency. Furthermore, we know this is
            // initialized because it is the value pointed at by `value.head`
            // and this is a non-empty queue.
            let val = unsafe {
                debug_assert!(index < value.buffer.len());
                let slot = value.buffer.get_unchecked_mut(index);
                slot.value.get().read().assume_init()
            };
            let new = if index + 1 < value.cap {
                // Same lap, incremented index.
                // Set to `{ lap: lap, index: index + 1 }`.
                head + 1
            } else {
                // One lap forward, index wraps around to zero.
                // Set to `{ lap: lap.wrapping_add(1), index: 0 }`.
                lap.wrapping_add(value.one_lap)
            };
            *value.head.get_mut() = new;
            Some(val)
        } else {
            None
        }
    }
}
