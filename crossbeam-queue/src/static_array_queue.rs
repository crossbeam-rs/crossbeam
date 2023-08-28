use core::fmt;
use core::mem::MaybeUninit;

use super::array_queue_impl::{ArrayQueueImpl, IntoIterImpl, Slot};

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
    /// The queue implementation.
    queue: ArrayQueueImpl<T, [Slot<T>; N]>,
}

unsafe impl<T: Send, const N: usize> Sync for StaticArrayQueue<T, N> {}

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

        let mut buffer = [const { MaybeUninit::<Slot<T>>::uninit() }; N];

        {
            // const for's are not stabilized yet, so use a loop
            let mut i = 0;
            while i < N {
                // Set the stamp to `{ lap: 0, index: i }`.
                buffer[i] = MaybeUninit::new(Slot::new(i));

                i += 1;
            }
        }

        // Allocate a buffer of `cap` slots initialized
        // with stamps.
        let buffer: [Slot<T>; N] = unsafe { MaybeUninit::array_assume_init(buffer) };

        StaticArrayQueue {
            queue: ArrayQueueImpl::new(buffer, N),
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
        self.queue.push(value)
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
        self.queue.force_push(value)
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
        self.queue.pop()
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
        self.queue.capacity()
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
        self.queue.is_empty()
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
        self.queue.is_full()
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
        self.queue.len()
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
        IntoIterImpl::new(&mut self.value.queue).next()
    }
}
