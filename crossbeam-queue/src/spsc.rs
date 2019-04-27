//! A bounded single-producer single-consumer queue.
//!
//! # Examples
//!
//! ```
//! use crossbeam_queue::spsc;
//!
//! let (p, c) = spsc::new(2);
//!
//! assert!(p.push(1).is_ok());
//! assert!(p.push(2).is_ok());
//! assert!(p.push(3).is_err());
//!
//! assert_eq!(c.pop(), Ok(1));
//! assert_eq!(c.pop(), Ok(2));
//! assert!(c.pop().is_err());
//! ```

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
    /// This integer is in range `0 .. 2 * cap`.
    head: CachePadded<AtomicUsize>,

    /// The tail of the queue.
    ///
    /// This integer is in range `0 .. 2 * cap`.
    tail: CachePadded<AtomicUsize>,

    /// The buffer holding slots.
    buffer: *mut T,

    /// The queue capacity.
    cap: usize,

    /// Indicates that dropping a `Buffer<T>` may drop elements of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Inner<T> {
    /// Returns a pointer to the slot at position `pos`.
    ///
    /// The position must be in range `0 .. 2 * cap`.
    #[inline]
    unsafe fn slot(&self, pos: usize) -> *mut T {
        if pos < self.cap {
            self.buffer.add(pos)
        } else {
            self.buffer.add(pos - self.cap)
        }
    }

    /// Increments a position by going one slot forward.
    ///
    /// The position must be in range `0 .. 2 * cap`.
    #[inline]
    fn increment(&self, pos: usize) -> usize {
        if pos < 2 * self.cap - 1 {
            pos + 1
        } else {
            0
        }
    }

    /// Returns the distance between two positions.
    ///
    /// Positions must be in range `0 .. 2 * cap`.
    #[inline]
    fn distance(&self, a: usize, b: usize) -> usize {
        if a <= b {
            b - a
        } else {
            2 * self.cap - a + b
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
/// let (p, c) = spsc::new::<i32>(100);
/// ```
pub fn new<T>(cap: usize) -> (Producer<T>, Consumer<T>) {
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
/// let (p, c) = spsc::new::<i32>(1);
///
/// assert_eq!(p.push(10), Ok(()));
/// assert_eq!(p.push(20), Err(PushError(20)));
/// ```
pub struct Producer<T> {
    /// The inner representation of the queue.
    inner: Arc<Inner<T>>,

    /// A copy of `inner.head` for quick access.
    ///
    /// This value can be stale and sometimes needs to be resynchronized with `inner.head`.
    head: Cell<usize>,

    /// A copy of `inner.tail` for quick access.
    ///
    /// This value is always in sync with `inner.tail`.
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
    /// let (p, c) = spsc::new(1);
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
    /// let (p, c) = spsc::new::<i32>(100);
    ///
    /// assert_eq!(p.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.inner.cap
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
/// let (p, c) = spsc::new(1);
/// assert_eq!(p.push(10), Ok(()));
///
/// assert_eq!(c.pop(), Ok(10));
/// assert_eq!(c.pop(), Err(PopError));
/// ```
pub struct Consumer<T> {
    /// The inner representation of the queue.
    inner: Arc<Inner<T>>,

    /// A copy of `inner.head` for quick access.
    ///
    /// This value is always in sync with `inner.head`.
    head: Cell<usize>,

    /// A copy of `inner.tail` for quick access.
    ///
    /// This value can be stale and sometimes needs to be resynchronized with `inner.tail`.
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
    /// let (p, c) = spsc::new(1);
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
    /// let (p, c) = spsc::new::<i32>(100);
    ///
    /// assert_eq!(c.capacity(), 100);
    /// ```
    pub fn capacity(&self) -> usize {
        self.inner.cap
    }
}

impl<T> fmt::Debug for Consumer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Consumer { .. }")
    }
}
