//! A concurrent work-stealing deque.
//!
//! This data structure is most commonly used in schedulers. The typical setup involves a number of
//! threads where each thread has its own deque containing tasks. A thread may push tasks into its
//! deque as well as pop tasks from it. Once it runs out of tasks, it may steal some from other
//! threads to help complete tasks more quickly. Therefore, work-stealing deques supports three
//! essential operations: *push*, *pop*, and *steal*.
//!
//! # Types of deques
//!
//! There are two types of deques, differing only in which order tasks get pushed and popped. The
//! two task ordering strategies are:
//!
//! * First-in first-out (FIFO)
//! * Last-in first-out (LIFO)
//!
//! A deque is a buffer with two ends, front and back. In a FIFO deque, tasks are pushed into the
//! back, popped from the front, and stolen from the front. However, in a LIFO deque, tasks are
//! popped from the back instead - that is the only difference.
//!
//! # Workers and stealers
//!
//! There are two functions that construct a deque: [`fifo`] and [`lifo`]. These functions return a
//! [`Worker`] and a [`Stealer`]. The thread which owns the deque is usually called *worker*, while
//! all other threads are *stealers*.
//!
//! [`Worker`] is able to push and pop tasks. It cannot be shared among multiple threads - only
//! one thread owns it.
//!
//! [`Stealer`] can only steal tasks. It can be shared among multiple threads by reference or by
//! cloning. Cloning a [`Stealer`] simply creates another one associated with the same deque.
//!
//! # Examples
//!
//! ```
//! use crossbeam_deque as deque;
//! use std::thread;
//!
//! // Create a LIFO deque.
//! let (w, s) = deque::lifo();
//!
//! // Push several elements into the back.
//! w.push(1);
//! w.push(2);
//! w.push(3);
//!
//! // This is a LIFO deque, which means an element is popped from the back.
//! // If it was a FIFO deque, `w.pop()` would return `Some(1)`.
//! assert_eq!(w.pop(), Some(3));
//!
//! // Create a stealer thread.
//! thread::spawn(move || {
//!     assert_eq!(s.steal(), Some(1));
//!     assert_eq!(s.steal(), Some(2));
//! }).join().unwrap();
//! ```
//!
//! [`Worker`]: struct.Worker.html
//! [`Stealer`]: struct.Stealer.html
//! [`fifo`]: fn.fifo.html
//! [`lifo`]: fn.lifo.html

extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;

use std::fmt;

mod fifo;
mod lifo;

/// Creates a work-stealing deque with the first-in first-out strategy.
///
/// Elements are pushed into the back, popped from the front, and stolen from the front. In other
/// words, the worker side behaves as a FIFO queue.
///
/// # Examples
///
/// ```
/// use crossbeam_deque as deque;
///
/// let (w, s) = deque::fifo::<i32>();
/// w.push(1);
/// w.push(2);
/// w.push(3);
///
/// assert_eq!(s.steal(), Some(1));
/// assert_eq!(w.pop(), Some(2));
/// assert_eq!(w.pop(), Some(3));
/// ```
pub fn fifo<T>() -> (Worker<T>, Stealer<T>) {
    let (w, s) = fifo::new();
    let w = Worker { flavor: WorkerFlavor::Fifo(w) };
    let s = Stealer { flavor: StealerFlavor::Fifo(s) };
    (w, s)
}

/// Creates a work-stealing deque with the last-in first-out strategy.
///
/// Elements are pushed into the back, popped from the back, and stolen from the front. In other
/// words, the worker side behaves as a LIFO stack.
///
/// # Examples
///
/// ```
/// use crossbeam_deque as deque;
///
/// let (w, s) = deque::lifo::<i32>();
/// w.push(1);
/// w.push(2);
/// w.push(3);
///
/// assert_eq!(s.steal(), Some(1));
/// assert_eq!(w.pop(), Some(3));
/// assert_eq!(w.pop(), Some(2));
/// ```
pub fn lifo<T>() -> (Worker<T>, Stealer<T>) {
    let (w, s) = lifo::new();
    let w = Worker { flavor: WorkerFlavor::Lifo(w) };
    let s = Stealer { flavor: StealerFlavor::Lifo(s) };
    (w, s)
}

enum WorkerFlavor<T> {
    Fifo(fifo::Worker<T>),
    Lifo(lifo::Worker<T>),
}

/// The worker side of a deque.
///
/// Workers push elements into the back and pop elements depending on the strategy:
///
/// * In FIFO deques, elements are popped from the front.
/// * In LIFO deques, elements are popped from the back.
///
/// A deque has only one worker. Workers are not intended to be shared among multiple threads.
pub struct Worker<T> {
    flavor: WorkerFlavor<T>,
}

unsafe impl<T: Send> Send for Worker<T> {}

impl<T> Worker<T> {
    /// Returns `true` if the deque is empty.
    ///
    /// ```
    /// use crossbeam_deque as deque;
    ///
    /// let (w, _) = deque::lifo();
    /// assert!(w.is_empty());
    /// w.push(1);
    /// assert!(!w.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match self.flavor {
            WorkerFlavor::Fifo(ref flavor) => flavor.is_empty(),
            WorkerFlavor::Lifo(ref flavor) => flavor.is_empty(),
        }
    }

    /// Pushes an element into the back of the deque.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque as deque;
    ///
    /// let (w, _) = deque::lifo();
    /// w.push(1);
    /// w.push(2);
    /// ```
    pub fn push(&self, value: T) {
        match self.flavor {
            WorkerFlavor::Fifo(ref flavor) => flavor.push(value),
            WorkerFlavor::Lifo(ref flavor) => flavor.push(value),
        }
    }

    /// Pops an element from the deque.
    ///
    /// Which end of the deque is used depends on the strategy:
    ///
    /// * If this is a FIFO deque, an element is popped from the front.
    /// * If this is a LIFO deque, an element is popped from the back.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque as deque;
    ///
    /// let (w, _) = deque::fifo();
    /// w.push(1);
    /// w.push(2);
    ///
    /// assert_eq!(w.pop(), Some(1));
    /// assert_eq!(w.pop(), Some(2));
    /// assert_eq!(w.pop(), None);
    /// ```
    pub fn pop(&self) -> Option<T> {
        match self.flavor {
            WorkerFlavor::Fifo(ref flavor) => flavor.pop(),
            WorkerFlavor::Lifo(ref flavor) => flavor.pop(),
        }
    }
}

impl<T> fmt::Debug for Worker<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Worker {{ ... }}")
    }
}

enum StealerFlavor<T> {
    Fifo(fifo::Stealer<T>),
    Lifo(lifo::Stealer<T>),
}

/// The stealer side of a deque.
///
/// Stealers can only steal elements from the front of the deque.
///
/// Stealers are cloneable so that they can be easily shared among multiple threads.
pub struct Stealer<T> {
    flavor: StealerFlavor<T>,
}

unsafe impl<T: Send> Send for Stealer<T> {}
unsafe impl<T: Send> Sync for Stealer<T> {}

impl<T> Stealer<T> {
    /// Returns `true` if the deque is empty.
    ///
    /// ```
    /// use crossbeam_deque as deque;
    ///
    /// let (w, s) = deque::lifo();
    /// assert!(s.is_empty());
    /// w.push(1);
    /// assert!(!s.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match self.flavor {
            StealerFlavor::Fifo(ref flavor) => flavor.is_empty(),
            StealerFlavor::Lifo(ref flavor) => flavor.is_empty(),
        }
    }

    /// Steals an element from the front of the deque.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque as deque;
    ///
    /// let (w, s) = deque::lifo();
    /// w.push(1);
    /// w.push(2);
    ///
    /// assert_eq!(s.steal(), Some(1));
    /// assert_eq!(s.steal(), Some(2));
    /// assert_eq!(s.steal(), None);
    /// ```
    pub fn steal(&self) -> Option<T> {
        match self.flavor {
            StealerFlavor::Fifo(ref flavor) => flavor.steal(),
            StealerFlavor::Lifo(ref flavor) => flavor.steal(),
        }
    }
}

impl<T> Clone for Stealer<T> {
    fn clone(&self) -> Stealer<T> {
        match self.flavor {
            StealerFlavor::Fifo(ref flavor) => Stealer {
                flavor: StealerFlavor::Fifo(flavor.clone()),
            },
            StealerFlavor::Lifo(ref flavor) => Stealer {
                flavor: StealerFlavor::Lifo(flavor.clone()),
            },
        }
    }
}

impl<T> fmt::Debug for Stealer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Stealer {{ ... }}")
    }
}
