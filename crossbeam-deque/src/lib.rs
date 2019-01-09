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
//! use crossbeam_deque::{Racy, Worker};
//! use std::thread;
//!
//! // Create a LIFO deque.
//! let w = Worker::new_lifo();
//! let s = w.stealer();
//!
//! // Push several elements into the back.
//! w.push(1);
//! w.push(2);
//! w.push(3);
//!
//! // This is a LIFO deque, which means an element is popped from the back.
//! // If it was a FIFO deque, `w.pop()` would return `Some(1)`.
//! assert_eq!(Racy::spin(|| w.pop()), Some(3));
//!
//! // Create a stealer thread.
//! thread::spawn(move || {
//!     assert_eq!(Racy::spin(|| s.steal_one()), Some(1));
//!     assert_eq!(Racy::spin(|| s.steal_one()), Some(2));
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

use std::cell::{Cell, UnsafeCell};
use std::cmp;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::{self, AtomicIsize, AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use epoch::{Atomic, Owned};
use utils::CachePadded;

// TODO: rename `value` to `task`?

/// Minimum buffer capacity for a deque.
const MIN_CAP: usize = 32;

/// Maximum number of additional elements that can be stolen in `steal_batch`.
const MAX_BATCH: usize = 128;

/// If a buffer of at least this size is retired, thread-local garbage is flushed so that it gets
/// deallocated as soon as possible.
const FLUSH_THRESHOLD_BYTES: usize = 1 << 10;

/// TODO
#[must_use]
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Racy<T> {
    /// TODO
    Done(T),

    /// TODO
    Retry,
}

impl<T> Racy<T> {
    /// TODO
    pub fn spin<F>(mut f: F) -> T
    where
        F: FnMut() -> Racy<T>,
    {
        let mut backoff = Backoff::new();
        loop {
            match f() {
                Racy::Done(v) => return v,
                Racy::Retry => backoff.spin(),
            }
        }
    }
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
    /// Creates a work-stealing deque with the first-in first-out strategy.
    ///
    /// Elements are pushed into the back, popped from the front, and stolen from the front. In other
    /// words, the worker side behaves as a FIFO queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{Racy, Worker};
    ///
    /// let w = Worker::new_fifo();
    /// let s = w.stealer();
    ///
    /// w.push(1);
    /// w.push(2);
    /// w.push(3);
    ///
    /// assert_eq!(Racy::spin(|| s.steal_one()), Some(1));
    /// assert_eq!(Racy::spin(|| w.pop()), Some(2));
    /// assert_eq!(Racy::spin(|| w.pop()), Some(3));
    /// ```
    pub fn new_fifo() -> Worker<T> {
        let buffer = Buffer::alloc(MIN_CAP);

        let inner = Arc::new(CachePadded::new(Inner {
            front: AtomicIsize::new(0),
            back: AtomicIsize::new(0),
            buffer: Atomic::new(buffer),
        }));

        Worker {
            inner,
            cached_buffer: Cell::new(buffer),
            flavor: Flavor::Fifo,
            _marker: PhantomData,
        }
    }

    /// Creates a work-stealing deque with the last-in first-out strategy.
    ///
    /// Elements are pushed into the back, popped from the back, and stolen from the front. In other
    /// words, the worker side behaves as a LIFO stack.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{Racy, Worker};
    ///
    /// let w = Worker::new_lifo();
    /// let s = w.stealer();
    ///
    /// w.push(1);
    /// w.push(2);
    /// w.push(3);
    ///
    /// assert_eq!(Racy::spin(|| s.steal_one()), Some(1));
    /// assert_eq!(Racy::spin(|| w.pop()), Some(3));
    /// assert_eq!(Racy::spin(|| w.pop()), Some(2));
    /// ```
    pub fn new_lifo() -> Worker<T> {
        let buffer = Buffer::alloc(MIN_CAP);

        let inner = Arc::new(CachePadded::new(Inner {
            front: AtomicIsize::new(0),
            back: AtomicIsize::new(0),
            buffer: Atomic::new(buffer),
        }));

        Worker {
            inner,
            cached_buffer: Cell::new(buffer),
            flavor: Flavor::Lifo,
            _marker: PhantomData,
        }
    }

    /// TODO
    pub fn stealer(&self) -> Stealer<T> {
        Stealer {
            inner: self.inner.clone(),
            flavor: self.flavor,
        }
    }

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
    /// use crossbeam_deque::Worker;
    ///
    /// let w = Worker::new_lifo();
    ///
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
    /// use crossbeam_deque::Worker;
    ///
    /// let w = Worker::new_lifo();
    ///
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
    /// use crossbeam_deque::{Racy, Worker};
    ///
    /// let w = Worker::new_fifo();
    /// let s = w.stealer();
    ///
    /// w.push(1);
    /// w.push(2);
    ///
    /// assert_eq!(Racy::spin(|| w.pop()), Some(1));
    /// assert_eq!(Racy::spin(|| w.pop()), Some(2));
    /// assert_eq!(Racy::spin(|| w.pop()), None);
    /// ```
    pub fn pop(&self) -> Racy<Option<T>> {
        // Load the back and front index.
        let b = self.inner.back.load(Ordering::Relaxed);
        let f = self.inner.front.load(Ordering::Relaxed);

        // Calculate the length of the deque.
        let len = b.wrapping_sub(f);

        // Is the deque empty?
        if len <= 0 {
            return Racy::Done(None);
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

                        return Racy::Done(Some(data));
                    }
                }

                Racy::Retry
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
                    Racy::Done(None)
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

                    Racy::Done(value)
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
    /// use crossbeam_deque::Worker;
    ///
    /// let w = Worker::new_lifo();
    /// let s = w.stealer();
    ///
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
    /// use crossbeam_deque::{Racy, Worker};
    ///
    /// let w = Worker::new_lifo();
    /// let s = w.stealer();
    ///
    /// w.push(1);
    /// w.push(2);
    ///
    /// assert_eq!(Racy::spin(|| s.steal_one()), Some(1));
    /// assert_eq!(Racy::spin(|| s.steal_one()), Some(2));
    /// assert_eq!(Racy::spin(|| s.steal_one()), None);
    /// ```
    pub fn steal_one(&self) -> Racy<Option<T>> {
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
            return Racy::Done(None);
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
            return Racy::Retry;
        }

        // Return the stolen value.
        Racy::Done(Some(value))
    }

    /// TODO
    pub fn steal_batch(&self, dest: &Worker<T>) -> Racy<bool> {
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
            return Racy::Done(false);
        }

        // Reserve capacity for the stolen batch.
        let batch_size = cmp::min((len as usize + 1) / 2, MAX_BATCH);
        dest.reserve(batch_size);
        let batch_size = batch_size as isize;

        // Get the destination buffer and back index.
        let dest_buffer = dest.cached_buffer.get();
        let mut dest_b = dest.inner.back.load(Ordering::Relaxed);

        // Load the buffer.
        let buffer = self.inner.buffer.load(Ordering::Acquire, guard);

        match self.flavor {
            // Steal a batch of elements from the front at once.
            Flavor::Fifo => {
                // Copy the batch from the source to the destination buffer.
                for i in 0..batch_size {
                    unsafe {
                        let value = buffer.deref().read(f.wrapping_add(i));
                        dest_buffer.write(dest_b.wrapping_add(i), value);
                    }
                }

                // Try incrementing the front index to steal the batch.
                if self
                    .inner
                    .front
                    .compare_exchange(
                        f,
                        f.wrapping_add(batch_size),
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_err()
                {
                    return Racy::Retry;
                }

                atomic::fence(Ordering::Release);

                // Success! Update the back index in the destination deque.
                //
                // This ordering could be `Relaxed`, but then thread sanitizer would falsely report
                // data races because it doesn't understand fences.
                dest.inner
                    .back
                    .store(dest_b.wrapping_add(batch_size), Ordering::Release);

                // Return with success.
                Racy::Done(true)
            }

            // Steal a batch of elements from the front one by one.
            Flavor::Lifo => {
                for _ in 0..batch_size {
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

                // Return with success.
                Racy::Done(true)
            }
        }
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
    /// use crossbeam_deque::{Racy, Worker};
    ///
    /// let w1 = Worker::new_fifo();
    /// let s1 = w1.stealer();
    ///
    /// w1.push(1);
    /// w1.push(2);
    /// w1.push(3);
    /// w1.push(4);
    ///
    /// let w2 = Worker::new_fifo();
    /// let s2 = w2.stealer();
    ///
    /// assert_eq!(Racy::spin(|| s1.steal_one_and_batch(&w2)), Some(1));
    /// assert_eq!(Racy::spin(|| s2.steal_one()), Some(2));
    /// ```
    pub fn steal_one_and_batch(&self, dest: &Worker<T>) -> Racy<Option<T>> {
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
            return Racy::Done(None);
        }

        // Reserve capacity for the stolen batch.
        let batch_size = cmp::min((len as usize - 1) / 2, MAX_BATCH);
        dest.reserve(batch_size);
        let batch_size = batch_size as isize;

        // Get the destination buffer and back index.
        let dest_buffer = dest.cached_buffer.get();
        let mut dest_b = dest.inner.back.load(Ordering::Relaxed);

        // Load the buffer and read the value at the front.
        let buffer = self.inner.buffer.load(Ordering::Acquire, guard);
        let value = unsafe { buffer.deref().read(f) };

        match self.flavor {
            // Steal a batch of elements from the front at once.
            Flavor::Fifo => {
                // Copy the batch from the source to the destination buffer.
                for i in 0..batch_size {
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
                        f.wrapping_add(batch_size + 1),
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    ).is_err()
                {
                    // We didn't steal this value, forget it.
                    mem::forget(value);
                    return Racy::Retry;
                }

                atomic::fence(Ordering::Release);

                // Success! Update the back index in the destination deque.
                //
                // This ordering could be `Relaxed`, but then thread sanitizer would falsely report
                // data races because it doesn't understand fences.
                dest.inner
                    .back
                    .store(dest_b.wrapping_add(batch_size), Ordering::Release);

                // Return the first stolen value.
                Racy::Done(Some(value))
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
                    return Racy::Retry;
                }

                // Move the front index one step forward.
                f = f.wrapping_add(1);

                // Repeat the same procedure for the batch steals.
                for _ in 0..batch_size {
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
                Racy::Done(Some(value))
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
// How many lower bits are reserved for metadata.
const SHIFT: usize = 1;
// Indicates that the block is not the last one.
const HAS_NEXT: usize = 1;

/// A slot in a block.
struct Slot<T> {
    /// The value.
    value: UnsafeCell<ManuallyDrop<T>>,

    /// The state of the slot.
    state: AtomicUsize,
}

impl<T> Slot<T> {
    /// Waits until a value is written into the slot.
    fn wait_write(&self) {
        let mut backoff = Backoff::new();
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
        unsafe { mem::zeroed() }
    }

    /// Waits until the next pointer is set.
    fn wait_next(&self) -> *mut Block<T> {
        let mut backoff = Backoff::new();
        loop {
            let next = self.next.load(Ordering::Acquire);
            if !next.is_null() {
                return next;
            }
            backoff.snooze();
        }
    }

    /// Sets the `DESTROY` bit in slots starting from `start` and destroys the block.
    unsafe fn destroy(this: *mut Block<T>, start: usize) {
        // It is not necessary to set the `DESTROY bit in the last slot because that slot has begun
        // destruction of the block.
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

        // No thread is using the block, now it is safe to destroy it.
        drop(Box::from_raw(this));
    }
}

/// A position in a queue.
#[derive(Debug)]
struct Position<T> {
    /// The index in the queue.
    index: AtomicUsize,

    /// The block in the linked list.
    block: AtomicPtr<Block<T>>,
}

/// Unbounded queue implemented as a linked list of blocks.
///
/// Each value sent into the queue is assigned a sequence number, i.e. an index. Indices are
/// represented as numbers of type `usize` and wrap on overflow.
///
/// Consecutive values are grouped into blocks in order to put less pressure on the allocator and
/// improve cache efficiency.
#[derive(Debug)]
pub struct Injector<T> {
    /// The head of the queue.
    head: CachePadded<Position<T>>,

    /// The tail of the queue.
    tail: CachePadded<Position<T>>,

    /// Indicates that dropping a `Injector<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Injector<T> {}
unsafe impl<T: Send> Sync for Injector<T> {}

impl<T> Injector<T> {
    /// Creates a new injector queue.
    pub fn new() -> Self {
        let block = Box::into_raw(Box::new(Block::<T>::new()));
        Injector {
            head: CachePadded::new(Position {
                block: AtomicPtr::new(block),
                index: AtomicUsize::new(0),
            }),
            tail: CachePadded::new(Position {
                block: AtomicPtr::new(block),
                index: AtomicUsize::new(0),
            }),
            _marker: PhantomData,
        }
    }

    /// TODO
    pub fn push(&self, value: T) {
        let mut backoff = Backoff::new();
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

            // If we're going to have to install the next block, allocate it in advance in order to
            // make the wait for other threads as short as possible.
            if offset + 1 == BLOCK_CAP && next_block.is_none() {
                next_block = Some(Box::new(Block::<T>::new()));
            }

            let new_tail = tail + (1 << SHIFT);

            // Try advancing the tail forward.
            match self.tail.index
                .compare_exchange_weak(
                    tail,
                    new_tail,
                    Ordering::SeqCst,
                    Ordering::Acquire,
                )
            {
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
                    slot.value.get().write(ManuallyDrop::new(value));
                    slot.state.fetch_or(WRITE, Ordering::Release);

                    return;
                }
                Err(t) => {
                    tail = t;
                    block = self.tail.block.load(Ordering::Acquire);
                    backoff.spin();
                }
            }
        }
    }

    /// TODO
    pub fn pop(&self) -> Racy<Option<T>> {
        let head = self.head.index.load(Ordering::Acquire);
        let block = self.head.block.load(Ordering::Acquire);

        // Calculate the offset of the index into the block.
        let offset = (head >> SHIFT) % LAP;

        // If we reached the end of the block, wait until the next one is installed.
        if offset == BLOCK_CAP {
            return Racy::Retry;
        }

        let mut new_head = head + (1 << SHIFT);

        if new_head & HAS_NEXT == 0 {
            atomic::fence(Ordering::SeqCst);
            let tail = self.tail.index.load(Ordering::Relaxed);

            // If the tail equals the head, that means the queue is empty.
            if head >> SHIFT == tail >> SHIFT {
                return Racy::Done(None);
            }

            // If head and tail are not in the same block, set `HAS_NEXT` in head.
            if (head >> SHIFT) / LAP != (tail >> SHIFT) / LAP {
                new_head |= HAS_NEXT;
            }
        }

        // Try moving the head index forward.
        if self.head.index
            .compare_exchange_weak(
                head,
                new_head,
                Ordering::SeqCst,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Racy::Retry;
        }

        unsafe {
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
            let m = slot.value.get().read();
            let value = ManuallyDrop::into_inner(m);

            // Destroy the block if we've reached the end, or if another thread wanted to destroy
            // but couldn't because we were busy reading from the slot.
            if offset + 1 == BLOCK_CAP {
                Block::destroy(block, 0);
            } else if slot.state.fetch_or(READ, Ordering::AcqRel) & DESTROY != 0 {
                Block::destroy(block, offset + 1);
            }

            Racy::Done(Some(value))
        }
    }

    /// Returns `true` if the queue is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.index.load(Ordering::SeqCst);
        let tail = self.tail.index.load(Ordering::SeqCst);
        head >> SHIFT == tail >> SHIFT
    }

    /// Returns the current number of elements in the queue.
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

                // Rotate indices so that head falls into the first block.
                let lap = (head >> SHIFT) / LAP;
                tail = tail.wrapping_sub((lap * LAP) << SHIFT);
                head = head.wrapping_sub((lap * LAP) << SHIFT);

                // Remove the lower bits.
                tail >>= SHIFT;
                head >>= SHIFT;

                // Fix up indices if they fall onto block ends.
                if head == BLOCK_CAP {
                    head = 0;
                    tail -= LAP;
                }
                if tail == BLOCK_CAP {
                    tail += 1;
                }

                // Return the difference minus the number of blocks between tail and head.
                return tail - head - tail / LAP;
            }
        }
    }
}

impl<T> Drop for Injector<T> {
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
                    ManuallyDrop::drop(&mut *(*slot).value.get());
                } else {
                    // Deallocate the block and move to the next one.
                    let next = (*block).next.load(Ordering::Relaxed);
                    drop(Box::from_raw(block));
                    block = next;
                }

                head = head.wrapping_add(1 << SHIFT);
            }

            // Deallocate the last remaining block.
            if !block.is_null() {
                drop(Box::from_raw(block));
            }
        }
    }
}

struct Backoff(u32);

impl Backoff {
    /// Creates a new `Backoff`.
    #[inline]
    pub fn new() -> Self {
        Backoff(0)
    }

    /// Backs off in a spin loop.
    ///
    /// This method may yield the current processor. Use it in lock-free retry loops.
    #[inline]
    pub fn spin(&mut self) {
        for _ in 0..1 << self.0.min(6) {
            atomic::spin_loop_hint();
        }
        self.0 = self.0.wrapping_add(1);
    }

    /// Backs off in a wait loop.
    ///
    /// Returns `true` if snoozing has reached a threshold where we should consider parking the
    /// thread instead.
    ///
    /// This method may yield the current processor or the current thread. Use it when waiting on a
    /// resource.
    #[inline]
    pub fn snooze(&mut self) -> bool {
        if self.0 <= 6 {
            for _ in 0..1 << self.0 {
                atomic::spin_loop_hint();
            }
        } else {
            thread::yield_now();
        }

        self.0 = self.0.wrapping_add(1);
        self.0 <= 10
    }
}
