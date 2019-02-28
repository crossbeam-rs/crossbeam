use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crossbeam_utils::CachePadded;

use err::{PopError, PushError};

/// The inner representation of a single-producer single-consumer queue.
struct Inner<T> {
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
    buffer: *mut T,

    /// The queue capacity.
    cap: usize,

    /// A stamp with the value of `{ lap: 1, index: 0 }`.
    one_lap: usize,

    /// Indicates that dropping a `Buffer<T>` may drop elements of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Inner<T> {
    /// Returns a pointer to the slot at position `pos`.
    ///
    /// The position is a stamp containing an index and a lap.
    unsafe fn slot(&self, pos: usize) -> *mut T {
        self.buffer.add(pos & (self.one_lap - 1))
    }

    /// Increments a position by going one slot forward.
    ///
    /// The position is a stamp containing an index and a lap.
    fn increment(&self, pos: usize) -> usize {
        let index = pos & (self.one_lap - 1);
        let lap = pos & !(self.one_lap - 1);

        if index < self.cap - 1 {
            pos + 1
        } else {
            lap.wrapping_add(self.one_lap)
        }
    }

    /// Returns the distance between two positions.
    ///
    /// The positions are stamps containing an index and a lap.
    fn distance(&self, a: usize, b: usize) -> usize {
        let index_a = a & (self.one_lap - 1);
        let lap_a = a & !(self.one_lap - 1);

        let index_b = b & (self.one_lap - 1);
        let lap_b = b & !(self.one_lap - 1);

        if lap_a == lap_b {
            index_b - index_a
        } else {
            self.cap - index_a + index_b
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let mut head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);

        // Loop over all slots that hold a value and drop them.
        while head != tail {
            unsafe {
                self.slot(head).drop_in_place();
            }
            head = self.increment(head);
        }

        // Finally, deallocate the buffer, but don't run any destructors.
        unsafe {
            Vec::from_raw_parts(self.buffer, 0, self.cap);
        }
    }
}

/// Creates a bounded single-producer single-consumer queue with the given capacity.
///
/// Returns the producer and the consumer side for the queue.
///
/// # Panics
///
/// Panics if the capacity is zero.
///
/// # Examples
///
/// ```
/// use crossbeam_queue::spsc;
///
/// let (p, c) = spsc::<i32>(100);
/// ```
pub fn spsc<T>(cap: usize) -> (Producer<T>, Consumer<T>) {
    assert!(cap > 0, "capacity must be non-zero");

    // Allocate a buffer of length `cap`.
    let buffer = {
        let mut v = Vec::<T>::with_capacity(cap);
        let ptr = v.as_mut_ptr();
        mem::forget(v);
        ptr
    };

    let inner = Arc::new(Inner {
        head: CachePadded::new(AtomicUsize::new(0)),
        tail: CachePadded::new(AtomicUsize::new(0)),
        buffer,
        cap,
        one_lap: cap.next_power_of_two(),
        _marker: PhantomData,
    });

    let p = Producer {
        inner: inner.clone(),
        head: Cell::new(0),
        tail: Cell::new(0),
    };

    let c = Consumer {
        inner,
        head: Cell::new(0),
        tail: Cell::new(0),
    };

    (p, c)
}

/// The producer side of a bounded single-producer single-consumer queue.
///
/// # Examples
///
/// ```
/// use crossbeam_queue::{spsc, PushError};
///
/// let (p, c) = spsc::<i32>(1);
///
/// assert_eq!(p.push(10), Ok(()));
/// assert_eq!(p.push(20), Err(PushError(20)));
///
/// assert!(!p.is_empty());
/// assert!(p.is_full());
/// ```
pub struct Producer<T> {
    inner: Arc<Inner<T>>,
    head: Cell<usize>,
    tail: Cell<usize>,
}

unsafe impl<T: Send> Send for Producer<T> {}

impl<T> Producer<T> {
    /// Attempts to push an element into the queue.
    ///
    /// If the queue is full, the element is returned back as an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::{spsc, PushError};
    ///
    /// let (p, c) = spsc(1);
    ///
    /// assert_eq!(p.push(10), Ok(()));
    /// assert_eq!(p.push(20), Err(PushError(20)));
    /// ```
    pub fn push(&self, value: T) -> Result<(), PushError<T>> {
        let mut head = self.head.get();
        let mut tail = self.tail.get();

        // Check if the queue is *possibly* full.
        if self.inner.distance(head, tail) == self.inner.cap {
            // We need to refresh the head and check again if the queue is *really* full.
            head = self.inner.head.load(Ordering::Acquire);
            self.head.set(head);

            // Is the queue *really* full?
            if self.inner.distance(head, tail) == self.inner.cap {
                return Err(PushError(value));
            }
        }

        // Write the value into the tail slot.
        unsafe {
            self.inner.slot(tail).write(value);
        }

        // Move the tail one slot forward.
        tail = self.inner.increment(tail);
        self.inner.tail.store(tail, Ordering::Release);
        self.tail.set(tail);

        Ok(())
    }

    /// Returns the capacity of the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc::<i32>(100);
    ///
    /// assert_eq!(p.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.inner.cap
    }

    /// Returns `true` if the queue is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc(100);
    ///
    /// assert!(p.is_empty());
    /// p.push(1).unwrap();
    /// assert!(!p.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if the queue is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc(1);
    ///
    /// assert!(!p.is_full());
    /// p.push(1).unwrap();
    /// assert!(p.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.len() == self.inner.cap
    }

    /// Returns the number of elements in the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc(100);
    /// assert_eq!(p.len(), 0);
    ///
    /// p.push(10).unwrap();
    /// assert_eq!(p.len(), 1);
    ///
    /// p.push(20).unwrap();
    /// assert_eq!(p.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        let head = self.inner.head.load(Ordering::Acquire);
        let tail = self.tail.get();
        self.head.set(head);
        self.inner.distance(head, tail)
    }
}

impl<T> fmt::Debug for Producer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Producer { .. }")
    }
}

/// The consumer side of a bounded single-producer single-consumer queue.
///
/// # Examples
///
/// ```
/// use crossbeam_queue::{spsc, PopError};
///
/// let (p, c) = spsc(1);
/// assert_eq!(p.push(10), Ok(()));
///
/// assert_eq!(c.pop(), Ok(10));
/// assert_eq!(c.pop(), Err(PopError));
///
/// assert!(c.is_empty());
/// assert!(!c.is_full());
/// ```
pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
    head: Cell<usize>,
    tail: Cell<usize>,
}

unsafe impl<T: Send> Send for Consumer<T> {}

impl<T> Consumer<T> {
    /// Attempts to pop an element from the queue.
    ///
    /// If the queue is empty, an error is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::{spsc, PopError};
    ///
    /// let (p, c) = spsc(1);
    /// assert_eq!(p.push(10), Ok(()));
    ///
    /// assert_eq!(c.pop(), Ok(10));
    /// assert_eq!(c.pop(), Err(PopError));
    /// ```
    pub fn pop(&self) -> Result<T, PopError> {
        let mut head = self.head.get();
        let mut tail = self.tail.get();

        // Check if the queue is *possibly* empty.
        if head == tail {
            // We need to refresh the tail and check again if the queue is *really* empty.
            tail = self.inner.tail.load(Ordering::Acquire);
            self.tail.set(tail);

            // Is the queue *really* empty?
            if head == tail {
                return Err(PopError);
            }
        }

        // Read the value from the head slot.
        let value = unsafe { self.inner.slot(head).read() };

        // Move the head one slot forward.
        head = self.inner.increment(head);
        self.inner.head.store(head, Ordering::Release);
        self.head.set(head);

        Ok(value)
    }

    /// Returns the capacity of the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc::<i32>(100);
    ///
    /// assert_eq!(c.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.inner.cap
    }

    /// Returns `true` if the queue is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc(100);
    ///
    /// assert!(c.is_empty());
    /// p.push(1).unwrap();
    /// assert!(!c.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if the queue is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc(1);
    ///
    /// assert!(!c.is_full());
    /// p.push(1).unwrap();
    /// assert!(c.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.len() == self.inner.cap
    }

    /// Returns the number of elements in the queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_queue::spsc;
    ///
    /// let (p, c) = spsc(100);
    /// assert_eq!(c.len(), 0);
    ///
    /// p.push(10).unwrap();
    /// assert_eq!(c.len(), 1);
    ///
    /// p.push(20).unwrap();
    /// assert_eq!(c.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        let head = self.head.get();
        let tail = self.inner.tail.load(Ordering::Acquire);
        self.tail.set(tail);
        self.inner.distance(head, tail)
    }
}

impl<T> fmt::Debug for Consumer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Consumer { .. }")
    }
}
