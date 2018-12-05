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
//! use crossbeam_deque::{self as deque, Pop, Steal};
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
//! assert_eq!(w.pop(), Pop::Data(3));
//!
//! // Create a stealer thread.
//! thread::spawn(move || {
//!     assert_eq!(s.steal(), Steal::Data(1));
//!     assert_eq!(s.steal(), Steal::Data(2));
//! }).join().unwrap();
//! ```
//!
//! [`Worker`]: struct.Worker.html
//! [`Stealer`]: struct.Stealer.html
//! [`fifo`]: fn.fifo.html
//! [`lifo`]: fn.lifo.html

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;

use std::cell::Cell;
use std::cmp;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicIsize, Ordering};
use std::sync::Arc;

use epoch::{Atomic, Owned};
use utils::CachePadded;

/// Minimum buffer capacity for a deque.
const MIN_CAP: usize = 32;

/// Maximum number of additional elements that can be stolen in `steal_many`.
const MAX_BATCH: usize = 128;

/// If a buffer of at least this size is retired, thread-local garbage is flushed so that it gets
/// deallocated as soon as possible.
const FLUSH_THRESHOLD_BYTES: usize = 1 << 10;

/// Creates a work-stealing deque with the first-in first-out strategy.
///
/// Elements are pushed into the back, popped from the front, and stolen from the front. In other
/// words, the worker side behaves as a FIFO queue.
///
/// # Examples
///
/// ```
/// use crossbeam_deque::{self as deque, Pop, Steal};
///
/// let (w, s) = deque::fifo::<i32>();
/// w.push(1);
/// w.push(2);
/// w.push(3);
///
/// assert_eq!(s.steal(), Steal::Data(1));
/// assert_eq!(w.pop(), Pop::Data(2));
/// assert_eq!(w.pop(), Pop::Data(3));
/// ```
pub fn fifo<T>() -> (Worker<T>, Stealer<T>) {
    let buffer = Buffer::alloc(MIN_CAP);

    let inner = Arc::new(CachePadded::new(Inner {
        front: AtomicIsize::new(0),
        back: AtomicIsize::new(0),
        buffer: Atomic::new(buffer),
    }));

    let w = Worker {
        inner: inner.clone(),
        cached_buffer: Cell::new(buffer),
        flavor: Flavor::Fifo,
        _marker: PhantomData,
    };
    let s = Stealer {
        inner,
        flavor: Flavor::Fifo,
    };
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
/// use crossbeam_deque::{self as deque, Pop, Steal};
///
/// let (w, s) = deque::lifo::<i32>();
/// w.push(1);
/// w.push(2);
/// w.push(3);
///
/// assert_eq!(s.steal(), Steal::Data(1));
/// assert_eq!(w.pop(), Pop::Data(3));
/// assert_eq!(w.pop(), Pop::Data(2));
/// ```
pub fn lifo<T>() -> (Worker<T>, Stealer<T>) {
    let buffer = Buffer::alloc(MIN_CAP);

    let inner = Arc::new(CachePadded::new(Inner {
        front: AtomicIsize::new(0),
        back: AtomicIsize::new(0),
        buffer: Atomic::new(buffer),
    }));

    let w = Worker {
        inner: inner.clone(),
        cached_buffer: Cell::new(buffer),
        flavor: Flavor::Lifo,
        _marker: PhantomData,
    };
    let s = Stealer {
        inner,
        flavor: Flavor::Lifo,
    };
    (w, s)
}

/// A buffer that holds elements in a deque.
///
/// This is just a pointer to the buffer and its length - dropping an instance of this struct will
/// *not* deallocate the buffer.
struct Buffer<T> {
    /// Pointer to the allocated memory.
    ptr: *mut T,

    /// Capacity of the buffer. Always a power of two.
    cap: usize,
}

unsafe impl<T> Send for Buffer<T> {}

impl<T> Buffer<T> {
    /// Allocates a new buffer with the specified capacity.
    fn alloc(cap: usize) -> Self {
        debug_assert_eq!(cap, cap.next_power_of_two());

        let mut v = Vec::with_capacity(cap);
        let ptr = v.as_mut_ptr();
        mem::forget(v);

        Buffer { ptr, cap }
    }

    /// Deallocates the buffer.
    unsafe fn dealloc(self) {
        drop(Vec::from_raw_parts(self.ptr, 0, self.cap));
    }

    /// Returns a pointer to the element at the specified `index`.
    unsafe fn at(&self, index: isize) -> *mut T {
        // `self.cap` is always a power of two.
        self.ptr.offset(index & (self.cap - 1) as isize)
    }

    /// Writes `value` into the specified `index`.
    ///
    /// Using this concurrently with another `read` or `write` is technically
    /// speaking UB due to data races.  We should be using relaxed accesses, but
    /// that would cost too much performance.  Hence, as a HACK, we use volatile
    /// accesses instead.  Experimental evidence shows that this works.
    unsafe fn write(&self, index: isize, value: T) {
        ptr::write_volatile(self.at(index), value)
    }

    /// Reads a value from the specified `index`.
    ///
    /// Using this concurrently with a `write` is technically speaking UB due to
    /// data races.  We should be using relaxed accesses, but that would cost
    /// too much performance.  Hence, as a HACK, we use volatile accesses
    /// instead.  Experimental evidence shows that this works.
    unsafe fn read(&self, index: isize) -> T {
        ptr::read_volatile(self.at(index))
    }
}

impl<T> Clone for Buffer<T> {
    fn clone(&self) -> Buffer<T> {
        Buffer {
            ptr: self.ptr,
            cap: self.cap,
        }
    }
}

impl<T> Copy for Buffer<T> {}

/// Possible outcomes of a pop operation.
#[must_use]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Pop<T> {
    /// The deque was empty at the time of popping.
    Empty,

    /// Some data has been successfully popped.
    Data(T),

    /// Lost the race for popping data to another concurrent steal operation. Try again.
    Retry,
}

/// Possible outcomes of a steal operation.
#[must_use]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Steal<T> {
    /// The deque was empty at the time of stealing.
    Empty,

    /// Some data has been successfully stolen.
    Data(T),

    /// Lost the race for stealing data to another concurrent steal or pop operation. Try again.
    Retry,
}

/// Internal data that is shared between the worker and stealers.
///
/// The implementation is based on the following work:
///
/// 1. [Chase and Lev. Dynamic circular work-stealing deque. SPAA 2005.][chase-lev]
/// 2. [Le, Pop, Cohen, and Nardelli. Correct and efficient work-stealing for weak memory models.
///    PPoPP 2013.][weak-mem]
/// 3. [Norris and Demsky. CDSchecker: checking concurrent data structures written with C/C++
///    atomics. OOPSLA 2013.][checker]
///
/// [chase-lev]: https://dl.acm.org/citation.cfm?id=1073974
/// [weak-mem]: https://dl.acm.org/citation.cfm?id=2442524
/// [checker]: https://dl.acm.org/citation.cfm?id=2509514
struct Inner<T> {
    /// The front index.
    front: AtomicIsize,

    /// The back index.
    back: AtomicIsize,

    /// The underlying buffer.
    buffer: Atomic<Buffer<T>>,
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Load the back index, front index, and buffer.
        let b = self.back.load(Ordering::Relaxed);
        let f = self.front.load(Ordering::Relaxed);

        unsafe {
            let buffer = self.buffer.load(Ordering::Relaxed, epoch::unprotected());

            // Go through the buffer from front to back and drop all elements in the deque.
            let mut i = f;
            while i != b {
                ptr::drop_in_place(buffer.deref().at(i));
                i = i.wrapping_add(1);
            }

            // Free the memory allocated by the buffer.
            buffer.into_owned().into_box().dealloc();
        }
    }
}

/// The flavor of a deque: FIFO or LIFO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flavor {
    /// The first-in first-out flavor.
    Fifo,

    /// The last-in first-out flavor.
    Lifo,
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
    /// A reference to the inner representation of the deque.
    inner: Arc<CachePadded<Inner<T>>>,

    /// A copy of `inner.buffer` for quick access.
    cached_buffer: Cell<Buffer<T>>,

    /// The flavor of the deque.
    flavor: Flavor,

    /// Indicates that the worker cannot be shared among threads.
    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for Worker<T> {}

impl<T> Worker<T> {
    /// Resizes the internal buffer to the new capacity of `new_cap`.
    #[cold]
    unsafe fn resize(&self, new_cap: usize) {
        // Load the back index, front index, and buffer.
        let b = self.inner.back.load(Ordering::Relaxed);
        let f = self.inner.front.load(Ordering::Relaxed);
        let buffer = self.cached_buffer.get();

        // Allocate a new buffer.
        let new = Buffer::alloc(new_cap);
        self.cached_buffer.set(new);

        // Copy data from the old buffer to the new one.
        let mut i = f;
        while i != b {
            ptr::copy_nonoverlapping(buffer.at(i), new.at(i), 1);
            i = i.wrapping_add(1);
        }

        let guard = &epoch::pin();

        // Replace the old buffer with the new one.
        let old =
            self.inner
                .buffer
                .swap(Owned::new(new).into_shared(guard), Ordering::Release, guard);

        // Destroy the old buffer later.
        guard.defer_unchecked(move || old.into_owned().into_box().dealloc());

        // If the buffer is very large, then flush the thread-local garbage in order to deallocate
        // it as soon as possible.
        if mem::size_of::<T>() * new_cap >= FLUSH_THRESHOLD_BYTES {
            guard.flush();
        }
    }

    /// Reserves enough capacity so that `reserve_cap` elements can be pushed without growing the
    /// buffer.
    fn reserve(&self, reserve_cap: usize) {
        if reserve_cap > 0 {
            // Compute the current length.
            let b = self.inner.back.load(Ordering::Relaxed);
            let f = self.inner.front.load(Ordering::SeqCst);
            let len = b.wrapping_sub(f) as usize;

            // The current capacity.
            let cap = self.cached_buffer.get().cap;

            // Is there enough capacity to push `reserve_cap` elements?
            if cap - len < reserve_cap {
                // Keep doubling the capacity as much as is needed.
                let mut new_cap = cap * 2;
                while new_cap - len < reserve_cap {
                    new_cap *= 2;
                }

                // Resize the buffer.
                unsafe {
                    self.resize(new_cap);
                }
            }
        }
    }

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
        let b = self.inner.back.load(Ordering::Relaxed);
        let f = self.inner.front.load(Ordering::SeqCst);
        b.wrapping_sub(f) <= 0
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
        // Load the back index, front index, and buffer.
        let b = self.inner.back.load(Ordering::Relaxed);
        let f = self.inner.front.load(Ordering::Acquire);
        let mut buffer = self.cached_buffer.get();

        // Calculate the length of the deque.
        let len = b.wrapping_sub(f);

        // Is the deque full?
        if len >= buffer.cap as isize {
            // Yes. Grow the underlying buffer.
            unsafe {
                self.resize(2 * buffer.cap);
            }
            buffer = self.cached_buffer.get();
        }

        // Write `value` into the slot.
        unsafe {
            buffer.write(b, value);
        }

        atomic::fence(Ordering::Release);

        // Increment the back index.
        //
        // This ordering could be `Relaxed`, but then thread sanitizer would falsely report data
        // races because it doesn't understand fences.
        self.inner.back.store(b.wrapping_add(1), Ordering::Release);
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
    /// use crossbeam_deque::{self as deque, Pop};
    ///
    /// let (w, _) = deque::fifo();
    /// w.push(1);
    /// w.push(2);
    ///
    /// assert_eq!(w.pop(), Pop::Data(1));
    /// assert_eq!(w.pop(), Pop::Data(2));
    /// assert_eq!(w.pop(), Pop::Empty);
    /// ```
    pub fn pop(&self) -> Pop<T> {
        // Load the back and front index.
        let b = self.inner.back.load(Ordering::Relaxed);
        let f = self.inner.front.load(Ordering::Relaxed);

        // Calculate the length of the deque.
        let len = b.wrapping_sub(f);

        // Is the deque empty?
        if len <= 0 {
            return Pop::Empty;
        }

        match self.flavor {
            // Pop from the front of the deque.
            Flavor::Fifo => {
                // Try incrementing the front index to pop the value.
                if self
                    .inner
                    .front
                    .compare_exchange(f, f.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    unsafe {
                        // Read the popped value.
                        let buffer = self.cached_buffer.get();
                        let data = buffer.read(f);

                        // Shrink the buffer if `len - 1` is less than one fourth of the capacity.
                        if buffer.cap > MIN_CAP && len <= buffer.cap as isize / 4 {
                            self.resize(buffer.cap / 2);
                        }

                        return Pop::Data(data);
                    }
                }

                Pop::Retry
            }

            // Pop from the back of the deque.
            Flavor::Lifo => {
                // Decrement the back index.
                let b = b.wrapping_sub(1);
                self.inner.back.store(b, Ordering::Relaxed);

                atomic::fence(Ordering::SeqCst);

                // Load the front index.
                let f = self.inner.front.load(Ordering::Relaxed);

                // Compute the length after the back index was decremented.
                let len = b.wrapping_sub(f);

                if len < 0 {
                    // The deque is empty. Restore the back index to the original value.
                    self.inner.back.store(b.wrapping_add(1), Ordering::Relaxed);
                    Pop::Empty
                } else {
                    // Read the value to be popped.
                    let buffer = self.cached_buffer.get();
                    let mut value = unsafe { Some(buffer.read(b)) };

                    // Are we popping the last element from the deque?
                    if len == 0 {
                        // Try incrementing the front index.
                        if self
                            .inner
                            .front
                            .compare_exchange(
                                f,
                                f.wrapping_add(1),
                                Ordering::SeqCst,
                                Ordering::Relaxed,
                            ).is_err()
                        {
                            // Failed. We didn't pop anything.
                            mem::forget(value.take());
                        }

                        // Restore the back index to the original value.
                        self.inner.back.store(b.wrapping_add(1), Ordering::Relaxed);
                    } else {
                        // Shrink the buffer if `len` is less than one fourth of the capacity.
                        if buffer.cap > MIN_CAP && len < buffer.cap as isize / 4 {
                            unsafe {
                                self.resize(buffer.cap / 2);
                            }
                        }
                    }

                    match value {
                        None => Pop::Empty,
                        Some(data) => Pop::Data(data),
                    }
                }
            }
        }
    }
}

impl<T> fmt::Debug for Worker<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Worker { .. }")
    }
}

/// The stealer side of a deque.
///
/// Stealers can only steal elements from the front of the deque.
///
/// Stealers are cloneable so that they can be easily shared among multiple threads.
pub struct Stealer<T> {
    /// A reference to the inner representation of the deque.
    inner: Arc<CachePadded<Inner<T>>>,

    /// The flavor of the deque.
    flavor: Flavor,
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
        let f = self.inner.front.load(Ordering::Acquire);
        atomic::fence(Ordering::SeqCst);
        let b = self.inner.back.load(Ordering::Acquire);
        b.wrapping_sub(f) <= 0
    }

    /// Steals an element from the front of the deque.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{self as deque, Steal};
    ///
    /// let (w, s) = deque::lifo();
    /// w.push(1);
    /// w.push(2);
    ///
    /// assert_eq!(s.steal(), Steal::Data(1));
    /// assert_eq!(s.steal(), Steal::Data(2));
    /// assert_eq!(s.steal(), Steal::Empty);
    /// ```
    pub fn steal(&self) -> Steal<T> {
        // Load the front index.
        let f = self.inner.front.load(Ordering::Acquire);

        // A SeqCst fence is needed here.
        //
        // If the current thread is already pinned (reentrantly), we must manually issue the
        // fence. Otherwise, the following pinning will issue the fence anyway, so we don't
        // have to.
        if epoch::is_pinned() {
            atomic::fence(Ordering::SeqCst);
        }

        let guard = &epoch::pin();

        // Load the back index.
        let b = self.inner.back.load(Ordering::Acquire);

        // Is the deque empty?
        if b.wrapping_sub(f) <= 0 {
            return Steal::Empty;
        }

        // Load the buffer and read the value at the front.
        let buffer = self.inner.buffer.load(Ordering::Acquire, guard);
        let value = unsafe { buffer.deref().read(f) };

        // Try incrementing the front index to steal the value.
        if self
            .inner
            .front
            .compare_exchange(f, f.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            // We didn't steal this value, forget it.
            mem::forget(value);
            return Steal::Retry;
        }

        // Return the stolen value.
        Steal::Data(value)
    }

    /// Steals elements from the front of the deque.
    ///
    /// If at least one element can be stolen, it will be returned. Additionally, some of the
    /// remaining elements will be stolen and pushed into the back of worker `dest` in order to
    /// balance the work among deques. There is no hard guarantee on exactly how many elements will
    /// be stolen, but it should be around half of the deque.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{self as deque, Steal};
    ///
    /// let (w1, s1) = deque::fifo();
    /// let (w2, s2) = deque::fifo();
    ///
    /// w1.push(1);
    /// w1.push(2);
    /// w1.push(3);
    /// w1.push(4);
    ///
    /// assert_eq!(s1.steal_many(&w2), Steal::Data(1));
    /// assert_eq!(s2.steal(), Steal::Data(2));
    /// ```
    pub fn steal_many(&self, dest: &Worker<T>) -> Steal<T> {
        // Load the front index.
        let mut f = self.inner.front.load(Ordering::Acquire);

        // A SeqCst fence is needed here.
        //
        // If the current thread is already pinned (reentrantly), we must manually issue the
        // fence. Otherwise, the following pinning will issue the fence anyway, so we don't
        // have to.
        if epoch::is_pinned() {
            atomic::fence(Ordering::SeqCst);
        }

        let guard = &epoch::pin();

        // Load the back index.
        let b = self.inner.back.load(Ordering::Acquire);

        // Is the deque empty?
        let len = b.wrapping_sub(f);
        if len <= 0 {
            return Steal::Empty;
        }

        // Reserve capacity for the stolen additional elements.
        let additional = cmp::min((len as usize - 1) / 2, MAX_BATCH);
        dest.reserve(additional);
        let additional = additional as isize;

        // Get the destination buffer and back index.
        let dest_buffer = dest.cached_buffer.get();
        let mut dest_b = dest.inner.back.load(Ordering::Relaxed);

        // Load the buffer and read the value at the front.
        let buffer = self.inner.buffer.load(Ordering::Acquire, guard);
        let value = unsafe { buffer.deref().read(f) };

        match self.flavor {
            // Steal a batch of elements from the front at once.
            Flavor::Fifo => {
                // Copy the additional elements from the source to the destination buffer.
                for i in 0..additional {
                    unsafe {
                        let value = buffer.deref().read(f.wrapping_add(i + 1));
                        dest_buffer.write(dest_b.wrapping_add(i), value);
                    }
                }

                // Try incrementing the front index to steal the batch.
                if self
                    .inner
                    .front
                    .compare_exchange(
                        f,
                        f.wrapping_add(additional + 1),
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_err()
                {
                    // We didn't steal this value, forget it.
                    mem::forget(value);
                    return Steal::Retry;
                }

                atomic::fence(Ordering::Release);

                // Success! Update the back index in the destination deque.
                //
                // This ordering could be `Relaxed`, but then thread sanitizer would falsely report
                // data races because it doesn't understand fences.
                dest.inner
                    .back
                    .store(dest_b.wrapping_add(additional), Ordering::Release);

                // Return the first stolen value.
                Steal::Data(value)
            }

            // Steal a batch of elements from the front one by one.
            Flavor::Lifo => {
                // Try incrementing the front index to steal the value.
                if self
                    .inner
                    .front
                    .compare_exchange(f, f.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
                    .is_err()
                {
                    // We didn't steal this value, forget it.
                    mem::forget(value);
                    return Steal::Retry;
                }

                // Move the front index one step forward.
                f = f.wrapping_add(1);

                // Repeat the same procedure for the additional steals.
                for _ in 0..additional {
                    // We've already got the current front index. Now execute the fence to
                    // synchronize with other threads.
                    atomic::fence(Ordering::SeqCst);

                    // Load the back index.
                    let b = self.inner.back.load(Ordering::Acquire);

                    // Is the deque empty?
                    if b.wrapping_sub(f) <= 0 {
                        break;
                    }

                    // Read the value at the front.
                    let value = unsafe { buffer.deref().read(f) };

                    // Try incrementing the front index to steal the value.
                    if self
                        .inner
                        .front
                        .compare_exchange(f, f.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
                        .is_err()
                    {
                        // We didn't steal this value, forget it and break from the loop.
                        mem::forget(value);
                        break;
                    }

                    // Write the stolen value into the destination buffer.
                    unsafe {
                        dest_buffer.write(dest_b, value);
                    }

                    // Move the source front index and the destination back index one step forward.
                    f = f.wrapping_add(1);
                    dest_b = dest_b.wrapping_add(1);

                    atomic::fence(Ordering::Release);

                    // Update the destination back index.
                    //
                    // This ordering could be `Relaxed`, but then thread sanitizer would falsely
                    // report data races because it doesn't understand fences.
                    dest.inner.back.store(dest_b, Ordering::Release);
                }

                // Return the first stolen value.
                Steal::Data(value)
            }
        }
    }
}

impl<T> Clone for Stealer<T> {
    fn clone(&self) -> Stealer<T> {
        Stealer {
            inner: self.inner.clone(),
            flavor: self.flavor,
        }
    }
}

impl<T> fmt::Debug for Stealer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Stealer { .. }")
    }
}
