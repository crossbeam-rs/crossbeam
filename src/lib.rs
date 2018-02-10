//! A concurrent work-stealing deque.
//!
//! The data structure can be thought of as a dynamically growable and shrinkable buffer that has
//! two ends: bottom and top. A [`Deque`] can [`push`] elements into the bottom and [`pop`]
//! elements from the bottom, but it can only [`steal`][Deque::steal] elements from the top.
//!
//! A [`Deque`] doesn't implement `Sync` so it cannot be shared among multiple threads. However, it
//! can create [`Stealer`]s, and those can be easily cloned, shared, and sent to other threads.
//! [`Stealer`]s can only [`steal`][Stealer::steal] elements from the top.
//!
//! Here's a visualization of the data structure:
//!
//! ```text
//!                    top
//!                     _
//!    Deque::steal -> | | <- Stealer::steal
//!                    | |
//!                    | |
//!                    | |
//! Deque::push/pop -> |_|
//!
//!                  bottom
//! ```
//!
//! # Work-stealing schedulers
//!
//! Usually, the data structure is used in work-stealing schedulers as follows.
//!
//! There is a number of threads. Each thread owns a [`Deque`] and creates a [`Stealer`] that is
//! shared among all other threads. Alternatively, it creates multiple [`Stealer`]s - one for each
//! of the other threads.
//!
//! Then, all threads are executing in a loop. In the loop, each one attempts to [`pop`] some work
//! from its own [`Deque`]. But if it is empty, it attempts to [`steal`][Stealer::steal] work from
//! some other thread instead. When executing work (or being idle), a thread may produce more work,
//! which gets [`push`]ed into its [`Deque`].
//!
//! Of course, there are many variations of this strategy. For example, sometimes it may be
//! beneficial for a thread to always [`steal`][Deque::steal] work from the top of its deque
//! instead of calling [`pop`] and taking it from the bottom.
//!
//! # Examples
//!
//! ```
//! use crossbeam_deque::{Deque, Steal};
//! use std::thread;
//!
//! let d = Deque::new();
//! let s = d.stealer();
//!
//! d.push('a');
//! d.push('b');
//! d.push('c');
//!
//! assert_eq!(d.pop(), Some('c'));
//! drop(d);
//!
//! thread::spawn(move || {
//!     assert_eq!(s.steal(), Steal::Data('a'));
//!     assert_eq!(s.steal(), Steal::Data('b'));
//! }).join().unwrap();
//! ```
//!
//! # References
//!
//! The implementation is based on the following work:
//!
//! 1. [Chase and Lev. Dynamic circular work-stealing deque. SPAA 2005.][chase-lev]
//! 2. [Le, Pop, Cohen, and Nardelli. Correct and efficient work-stealing for weak memory models.
//!    PPoPP 2013.][weak-mem]
//! 3. [Norris and Demsky. CDSchecker: checking concurrent data structures written with C/C++
//!    atomics. OOPSLA 2013.][checker]
//!
//! [chase-lev]: https://dl.acm.org/citation.cfm?id=1073974
//! [weak-mem]: https://dl.acm.org/citation.cfm?id=2442524
//! [checker]: https://dl.acm.org/citation.cfm?id=2509514
//!
//! [`Deque`]: struct.Deque.html
//! [`Stealer`]: struct.Stealer.html
//! [`push`]: struct.Deque.html#method.push
//! [`pop`]: struct.Deque.html#method.pop
//! [Deque::steal]: struct.Deque.html#method.steal
//! [Stealer::steal]: struct.Stealer.html#method.steal

extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;

use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicIsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};

use epoch::{Atomic, Owned};
use utils::cache_padded::CachePadded;

/// Minimum buffer capacity for a deque.
const DEFAULT_MIN_CAP: usize = 16;

/// If a buffer of at least this size is retired, thread-local garbage is flushed so that it gets
/// deallocated as soon as possible.
const FLUSH_THRESHOLD_BYTES: usize = 1 << 10;

/// Possible outcomes of a steal operation.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Steal<T> {
    /// The deque was empty at the time of stealing.
    Empty,

    /// Some data has been successfully stolen.
    Data(T),

    /// Lost the race for stealing data to another concurrent operation. Try again.
    Retry,
}

/// A buffer that holds elements in a deque.
struct Buffer<T> {
    /// Pointer to the allocated memory.
    ptr: *mut T,

    /// Capacity of the buffer. Always a power of two.
    cap: usize,
}

unsafe impl<T> Send for Buffer<T> {}

impl<T> Buffer<T> {
    /// Returns a new buffer with the specified capacity.
    fn new(cap: usize) -> Self {
        debug_assert_eq!(cap, cap.next_power_of_two());

        let mut v = Vec::with_capacity(cap);
        let ptr = v.as_mut_ptr();
        mem::forget(v);

        Buffer {
            ptr: ptr,
            cap: cap,
        }
    }

    /// Returns a pointer to the element at the specified `index`.
    unsafe fn at(&self, index: isize) -> *mut T {
        // `self.cap` is always a power of two.
        self.ptr.offset(index & (self.cap - 1) as isize)
    }

    /// Writes `value` into the specified `index`.
    unsafe fn write(&self, index: isize, value: T) {
        ptr::write(self.at(index), value)
    }

    /// Reads a value from the specified `index`.
    unsafe fn read(&self, index: isize) -> T {
        ptr::read(self.at(index))
    }
}

impl<T> Drop for Buffer<T> {
    fn drop(&mut self) {
        unsafe {
            drop(Vec::from_raw_parts(self.ptr, 0, self.cap));
        }
    }
}

/// Internal data that is shared between the deque and its stealers.
struct Inner<T> {
    /// The bottom index.
    bottom: AtomicIsize,

    /// The top index.
    top: AtomicIsize,

    /// The underlying buffer.
    buffer: Atomic<Buffer<T>>,

    /// Minimum capacity of the buffer. Always a power of two.
    min_cap: usize,
}

impl<T> Inner<T> {
    /// Returns a new `Inner` with default minimum capacity.
    fn new() -> Self {
        Self::with_min_capacity(DEFAULT_MIN_CAP)
    }

    /// Returns a new `Inner` with minimum capacity of `min_cap` rounded to the next power of two.
    fn with_min_capacity(min_cap: usize) -> Self {
        let power = min_cap.next_power_of_two();
        assert!(power >= min_cap, "capacity too large: {}", min_cap);
        Inner {
            bottom: AtomicIsize::new(0),
            top: AtomicIsize::new(0),
            buffer: Atomic::new(Buffer::new(power)),
            min_cap: power,
        }
    }

    /// Resizes the internal buffer to the new capacity of `new_cap`.
    #[cold]
    unsafe fn resize(&self, new_cap: usize) {
        // Load the bottom, top, and buffer.
        let b = self.bottom.load(Relaxed);
        let t = self.top.load(Relaxed);

        let buffer = self.buffer.load(Relaxed, epoch::unprotected());

        // Allocate a new buffer.
        let new = Buffer::new(new_cap);

        // Copy data from the old buffer to the new one.
        let mut i = t;
        while i != b {
            ptr::copy_nonoverlapping(buffer.deref().at(i), new.at(i), 1);
            i = i.wrapping_add(1);
        }

        let guard = &epoch::pin();

        // Replace the old buffer with the new one.
        let old = self.buffer
            .swap(Owned::new(new).into_shared(guard), Release, guard);

        // Destroy the old buffer later.
        guard.defer(move || old.into_owned());

        // If the buffer is very large, then flush the thread-local garbage in order to
        // deallocate it as soon as possible.
        if mem::size_of::<T>() * new_cap >= FLUSH_THRESHOLD_BYTES {
            guard.flush();
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Load the bottom, top, and buffer.
        let b = self.bottom.load(Relaxed);
        let t = self.top.load(Relaxed);

        unsafe {
            let buffer = self.buffer.load(Relaxed, epoch::unprotected());

            // Go through the buffer from top to bottom and drop all elements in the deque.
            let mut i = t;
            while i != b {
                ptr::drop_in_place(buffer.deref().at(i));
                i = i.wrapping_add(1);
            }

            // Free the memory allocated by the buffer.
            drop(buffer.into_owned());
        }
    }
}

/// A concurrent work-stealing deque.
///
/// A deque has two ends: bottom and top. Elements can be [`push`]ed into the bottom and [`pop`]ped
/// from the bottom. The top end is special in that elements can only be stolen from it using the
/// [`steal`][Deque::steal] method.
///
/// # Stealers
///
/// While [`Deque`] doesn't implement `Sync`, it can create [`Stealer`]s using the method
/// [`stealer`][stealer], and those can be easily shared among multiple threads. [`Stealer`]s can
/// only [`steal`][Stealer::steal] elements from the top end of the deque.
///
/// # Capacity
///
/// The data structure is dynamically grows as elements are inserted and removed from it. If the
/// internal buffer gets full, a new one twice the size of the original is allocated. Similarly,
/// if it is less than a quarter full, a new buffer half the size of the original is allocated.
///
/// In order to prevent frequent resizing (reallocations may be costly), it is possible to specify
/// a large minimum capacity for the deque by calling [`Deque::with_min_capacity`]. This
/// constructor will make sure that the internal buffer never shrinks below that size.
///
/// # Examples
///
/// ```
/// use crossbeam_deque::{Deque, Steal};
///
/// let d = Deque::with_min_capacity(1000);
/// let s = d.stealer();
///
/// d.push('a');
/// d.push('b');
/// d.push('c');
///
/// assert_eq!(d.pop(), Some('c'));
/// assert_eq!(d.steal(), Steal::Data('a'));
/// assert_eq!(s.steal(), Steal::Data('b'));
/// ```
///
/// [`Deque`]: struct.Deque.html
/// [`Stealer`]: struct.Stealer.html
/// [`push`]: struct.Deque.html#method.push
/// [`pop`]: struct.Deque.html#method.pop
/// [stealer]: struct.Deque.html#method.stealer
/// [`Deque::with_min_capacity`]: struct.Deque.html#method.with_min_capacity
/// [Deque::steal]: struct.Deque.html#method.steal
/// [Stealer::steal]: struct.Stealer.html#method.steal
pub struct Deque<T> {
    inner: Arc<CachePadded<Inner<T>>>,
    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for Deque<T> {}

impl<T> Deque<T> {
    /// Returns a new deque.
    ///
    /// The internal buffer is destructed as soon as the deque and all its stealers get dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::<i32>::new();
    /// ```
    pub fn new() -> Deque<T> {
        Deque {
            inner: Arc::new(CachePadded::new(Inner::new())),
            _marker: PhantomData,
        }
    }

    /// Returns a new deque with the specified minimum capacity.
    ///
    /// If the capacity is not a power of two, it will be rounded up to the next one.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// // The minimum capacity will be rounded up to 1024.
    /// let d = Deque::<i32>::with_min_capacity(1000);
    /// ```
    pub fn with_min_capacity(min_cap: usize) -> Deque<T> {
        Deque {
            inner: Arc::new(CachePadded::new(Inner::with_min_capacity(min_cap))),
            _marker: PhantomData,
        }
    }

    /// Returns `true` if the deque is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::new();
    /// assert!(d.is_empty());
    /// d.push("foo");
    /// assert!(!d.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of elements in the deque.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::new();
    /// d.push('a');
    /// d.push('b');
    /// d.push('c');
    /// assert_eq!(d.len(), 3);
    /// ```
    pub fn len(&self) -> usize {
        let b = self.inner.bottom.load(Relaxed);
        let t = self.inner.top.load(Relaxed);
        b.wrapping_sub(t) as usize
    }

    /// Pushes an element into the bottom of the deque.
    ///
    /// If the internal buffer is full, a new one twice the capacity of the current one will be
    /// allocated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::new();
    /// d.push(1);
    /// d.push(2);
    /// ```
    pub fn push(&self, value: T) {
        unsafe {
            // Load the bottom, top, and buffer. The buffer doesn't have to be epoch-protected
            // because the current thread (the worker) is the only one that grows and shrinks it.
            let b = self.inner.bottom.load(Relaxed);
            let t = self.inner.top.load(Acquire);

            let mut buffer = self.inner.buffer.load(Relaxed, epoch::unprotected());

            // Calculate the length of the deque.
            let len = b.wrapping_sub(t);

            // Is the deque full?
            let cap = buffer.deref().cap;
            if len >= cap as isize {
                // Yes. Grow the underlying buffer.
                self.inner.resize(2 * cap);
                buffer = self.inner.buffer.load(Relaxed, epoch::unprotected());
            }

            // Write `value` into the right slot and increment `b`.
            buffer.deref().write(b, value);
            atomic::fence(Release);
            self.inner.bottom.store(b.wrapping_add(1), Relaxed);
        }
    }

    /// Pops an element from the bottom of the deque.
    ///
    /// If the internal buffer is less than a quarter full, a new buffer half the capacity of the
    /// current one will be allocated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::new();
    /// d.push(1);
    /// d.push(2);
    ///
    /// assert_eq!(d.pop(), Some(2));
    /// assert_eq!(d.pop(), Some(1));
    /// assert_eq!(d.pop(), None);
    /// ```
    pub fn pop(&self) -> Option<T> {
        // Load the bottom.
        let b = self.inner.bottom.load(Relaxed);

        // If the deque is empty, return early without incurring the cost of a SeqCst fence.
        let t = self.inner.top.load(Relaxed);
        if b.wrapping_sub(t) <= 0 {
            return None;
        }

        // Decrement the bottom.
        let b = b.wrapping_sub(1);
        self.inner.bottom.store(b, Relaxed);

        // Load the buffer. The buffer doesn't have to be epoch-protected because the current
        // thread (the worker) is the only one that grows and shrinks it.
        let buf = unsafe { self.inner.buffer.load(Relaxed, epoch::unprotected()) };

        atomic::fence(SeqCst);

        // Load the top.
        let t = self.inner.top.load(Relaxed);

        // Compute the length after the bottom was decremented.
        let len = b.wrapping_sub(t);

        if len < 0 {
            // The deque is empty. Restore the bottom back to the original value.
            self.inner.bottom.store(b.wrapping_add(1), Relaxed);
            None
        } else {
            // Read the value to be popped.
            let mut value = unsafe { Some(buf.deref().read(b)) };

            // Are we popping the last element from the deque?
            if len == 0 {
                // Try incrementing the top.
                if self.inner
                    .top
                    .compare_exchange(t, t.wrapping_add(1), SeqCst, Relaxed)
                    .is_err()
                {
                    // Failed. We didn't pop anything.
                    mem::forget(value.take());
                }

                // Restore the bottom back to the original value.
                self.inner.bottom.store(b.wrapping_add(1), Relaxed);
            } else {
                // Shrink the buffer if `len` is less than one fourth of `self.inner.min_cap`.
                unsafe {
                    let cap = buf.deref().cap;
                    if cap > self.inner.min_cap && len < cap as isize / 4 {
                        self.inner.resize(cap / 2);
                    }
                }
            }

            value
        }
    }

    /// Steals an element from the top of the deque.
    ///
    /// Unlike most methods in concurrent data structures, if another operation gets in the way
    /// while attempting to steal data, this method will return immediately with [`Steal::Retry`]
    /// instead of retrying.
    ///
    /// If the internal buffer is less than a quarter full, a new buffer half the capacity of the
    /// current one will be allocated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{Deque, Steal};
    ///
    /// let d = Deque::new();
    /// d.push(1);
    /// d.push(2);
    ///
    /// // Attempt to steal an element.
    /// //
    /// // No other threads are working with the deque, so this time we know for sure that we
    /// // won't get `Steal::Retry` as the result.
    /// assert_eq!(d.steal(), Steal::Data(1));
    ///
    /// // Attempt to steal an element, but keep retrying if we get `Retry`.
    /// loop {
    ///     match d.steal() {
    ///         Steal::Empty => panic!("should steal something"),
    ///         Steal::Data(data) => {
    ///             assert_eq!(data, 2);
    ///             break;
    ///         }
    ///         Steal::Retry => {}
    ///     }
    /// }
    /// ```
    ///
    /// [`Steal::Retry`]: enum.Steal.html#variant.Retry
    pub fn steal(&self) -> Steal<T> {
        let b = self.inner.bottom.load(Relaxed);
        let buf = unsafe { self.inner.buffer.load(Relaxed, epoch::unprotected()) };
        let t = self.inner.top.load(Relaxed);
        let len = b.wrapping_sub(t);

        // Is the deque empty?
        if len <= 0 {
            return Steal::Empty;
        }

        // Try incrementing the top to steal the value.
        if self.inner
            .top
            .compare_exchange(t, t.wrapping_add(1), SeqCst, Relaxed)
            .is_ok()
        {
            let data = unsafe { buf.deref().read(t) };

            // Shrink the buffer if `len - 1` is less than one fourth of `self.inner.min_cap`.
            unsafe {
                let cap = buf.deref().cap;
                if cap > self.inner.min_cap && len <= cap as isize / 4 {
                    self.inner.resize(cap / 2);
                }
            }

            return Steal::Data(data);
        }

        Steal::Retry
    }

    /// Creates a stealer that can be shared with other threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{Deque, Steal};
    /// use std::thread;
    ///
    /// let d = Deque::new();
    /// d.push(1);
    /// d.push(2);
    ///
    /// let s = d.stealer();
    ///
    /// thread::spawn(move || {
    ///     assert_eq!(s.steal(), Steal::Data(1));
    /// }).join().unwrap();
    /// ```
    pub fn stealer(&self) -> Stealer<T> {
        Stealer {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for Deque<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Deque {{ ... }}")
    }
}

impl<T> Default for Deque<T> {
    fn default() -> Deque<T> {
        Deque::new()
    }
}

/// A stealer that steals elements from the top of a deque.
///
/// The only operation a stealer can do that manipulates the deque is [`steal`].
///
/// Stealers can be cloned in order to create more of them. They also implement `Send` and `Sync`
/// so they can be easily shared among multiple threads.
///
/// [`steal`]: struct.Stealer.html#method.steal
pub struct Stealer<T> {
    inner: Arc<CachePadded<Inner<T>>>,
    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for Stealer<T> {}
unsafe impl<T: Send> Sync for Stealer<T> {}

impl<T> Stealer<T> {
    /// Returns `true` if the deque is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::new();
    /// d.push("foo");
    ///
    /// let s = d.stealer();
    /// assert!(!d.is_empty());
    /// s.steal();
    /// assert!(d.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of elements in the deque.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::Deque;
    ///
    /// let d = Deque::new();
    /// let s = d.stealer();
    /// d.push('a');
    /// d.push('b');
    /// d.push('c');
    /// assert_eq!(s.len(), 3);
    /// ```
    pub fn len(&self) -> usize {
        let t = self.inner.top.load(Relaxed);
        atomic::fence(SeqCst);
        let b = self.inner.bottom.load(Relaxed);
        std::cmp::max(b.wrapping_sub(t), 0) as usize
    }

    /// Steals an element from the top of the deque.
    ///
    /// Unlike most methods in concurrent data structures, if another operation gets in the way
    /// while attempting to steal data, this method will return immediately with [`Steal::Retry`]
    /// instead of retrying.
    ///
    /// This method will not attempt to resize the internal buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_deque::{Deque, Steal};
    ///
    /// let d = Deque::new();
    /// let s = d.stealer();
    /// d.push(1);
    /// d.push(2);
    ///
    /// // Attempt to steal an element, but keep retrying if we get `Retry`.
    /// loop {
    ///     match d.steal() {
    ///         Steal::Empty => panic!("should steal something"),
    ///         Steal::Data(data) => {
    ///             assert_eq!(data, 1);
    ///             break;
    ///         }
    ///         Steal::Retry => {}
    ///     }
    /// }
    /// ```
    ///
    /// [`Steal::Retry`]: enum.Steal.html#variant.Retry
    pub fn steal(&self) -> Steal<T> {
        // Load the top.
        let t = self.inner.top.load(Acquire);

        // A SeqCst fence is needed here.
        // If the current thread is already pinned (reentrantly), we must manually issue the fence.
        // Otherwise, the following pinning will issue the fence anyway, so we don't have to.
        if epoch::is_pinned() {
            atomic::fence(SeqCst);
        }

        let guard = &epoch::pin();

        // Load the bottom.
        let b = self.inner.bottom.load(Acquire);

        // Is the deque empty?
        if b.wrapping_sub(t) <= 0 {
            return Steal::Empty;
        }

        // Load the buffer and read the value at the top.
        let buf = self.inner.buffer.load(Acquire, guard);
        let value = unsafe { buf.deref().read(t) };

        // Try incrementing the top to steal the value.
        if self.inner
            .top
            .compare_exchange(t, t.wrapping_add(1), SeqCst, Relaxed)
            .is_ok()
        {
            return Steal::Data(value);
        }

        // We didn't steal this value, forget it.
        mem::forget(value);

        Steal::Retry
    }
}

impl<T> Clone for Stealer<T> {
    /// Creates another stealer.
    fn clone(&self) -> Stealer<T> {
        Stealer {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for Stealer<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Stealer {{ ... }}")
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;

    use epoch;
    use self::rand::Rng;

    use super::{Deque, Steal};

    #[test]
    fn smoke() {
        let d = Deque::new();
        let s = d.stealer();
        assert_eq!(d.pop(), None);
        assert_eq!(s.steal(), Steal::Empty);
        assert_eq!(d.len(), 0);
        assert_eq!(s.len(), 0);

        d.push(1);
        assert_eq!(d.len(), 1);
        assert_eq!(s.len(), 1);
        assert_eq!(d.pop(), Some(1));
        assert_eq!(d.pop(), None);
        assert_eq!(s.steal(), Steal::Empty);
        assert_eq!(d.len(), 0);
        assert_eq!(s.len(), 0);

        d.push(2);
        assert_eq!(s.steal(), Steal::Data(2));
        assert_eq!(s.steal(), Steal::Empty);
        assert_eq!(d.pop(), None);

        d.push(3);
        d.push(4);
        d.push(5);
        assert_eq!(d.steal(), Steal::Data(3));
        assert_eq!(s.steal(), Steal::Data(4));
        assert_eq!(d.steal(), Steal::Data(5));
        assert_eq!(d.steal(), Steal::Empty);
    }

    #[test]
    fn steal_push() {
        const STEPS: usize = 50_000;

        let d = Deque::new();
        let s = d.stealer();
        let t = thread::spawn(move || for i in 0..STEPS {
            loop {
                if let Steal::Data(v) = s.steal() {
                    assert_eq!(i, v);
                    break;
                }
            }
        });

        for i in 0..STEPS {
            d.push(i);
        }
        t.join().unwrap();
    }

    #[test]
    fn stampede() {
        const COUNT: usize = 50_000;

        let d = Deque::new();

        for i in 0..COUNT {
            d.push(Box::new(i + 1));
        }
        let remaining = Arc::new(AtomicUsize::new(COUNT));

        let threads = (0..8)
            .map(|_| {
                let s = d.stealer();
                let remaining = remaining.clone();

                thread::spawn(move || {
                    let mut last = 0;
                    while remaining.load(SeqCst) > 0 {
                        if let Steal::Data(x) = s.steal() {
                            assert!(last < *x);
                            last = *x;
                            remaining.fetch_sub(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut last = COUNT + 1;
        while remaining.load(SeqCst) > 0 {
            if let Some(x) = d.pop() {
                assert!(last > *x);
                last = *x;
                remaining.fetch_sub(1, SeqCst);
            }
        }

        for t in threads {
            t.join().unwrap();
        }
    }

    fn run_stress() {
        const COUNT: usize = 50_000;

        let d = Deque::new();
        let done = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));

        let threads = (0..8)
            .map(|_| {
                let s = d.stealer();
                let done = done.clone();
                let hits = hits.clone();

                thread::spawn(move || while !done.load(SeqCst) {
                    if let Steal::Data(_) = s.steal() {
                        hits.fetch_add(1, SeqCst);
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut rng = rand::thread_rng();
        let mut expected = 0;
        while expected < COUNT {
            if rng.gen_range(0, 3) == 0 {
                if d.pop().is_some() {
                    hits.fetch_add(1, SeqCst);
                }
            } else {
                d.push(expected);
                expected += 1;
            }
        }

        while hits.load(SeqCst) < COUNT {
            if d.pop().is_some() {
                hits.fetch_add(1, SeqCst);
            }
        }
        done.store(true, SeqCst);

        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn stress() {
        run_stress();
    }

    #[test]
    fn stress_pinned() {
        let _guard = epoch::pin();
        run_stress();
    }

    #[test]
    fn no_starvation() {
        const COUNT: usize = 50_000;

        let d = Deque::new();
        let done = Arc::new(AtomicBool::new(false));

        let (threads, hits): (Vec<_>, Vec<_>) = (0..8)
            .map(|_| {
                let s = d.stealer();
                let done = done.clone();
                let hits = Arc::new(AtomicUsize::new(0));

                let t = {
                    let hits = hits.clone();
                    thread::spawn(move || while !done.load(SeqCst) {
                        if let Steal::Data(_) = s.steal() {
                            hits.fetch_add(1, SeqCst);
                        }
                    })
                };

                (t, hits)
            })
            .unzip();

        let mut rng = rand::thread_rng();
        let mut my_hits = 0;
        loop {
            for i in 0..rng.gen_range(0, COUNT) {
                if rng.gen_range(0, 3) == 0 && my_hits == 0 {
                    if d.pop().is_some() {
                        my_hits += 1;
                    }
                } else {
                    d.push(i);
                }
            }

            if my_hits > 0 && hits.iter().all(|h| h.load(SeqCst) > 0) {
                break;
            }
        }
        done.store(true, SeqCst);

        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn destructors() {
        const COUNT: usize = 50_000;

        struct Elem(usize, Arc<Mutex<Vec<usize>>>);

        impl Drop for Elem {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(self.0);
            }
        }

        let d = Deque::new();

        let dropped = Arc::new(Mutex::new(Vec::new()));
        let remaining = Arc::new(AtomicUsize::new(COUNT));
        for i in 0..COUNT {
            d.push(Elem(i, dropped.clone()));
        }

        let threads = (0..8)
            .map(|_| {
                let s = d.stealer();
                let remaining = remaining.clone();

                thread::spawn(move || for _ in 0..1000 {
                    if let Steal::Data(_) = s.steal() {
                        remaining.fetch_sub(1, SeqCst);
                    }
                })
            })
            .collect::<Vec<_>>();

        for _ in 0..1000 {
            if d.pop().is_some() {
                remaining.fetch_sub(1, SeqCst);
            }
        }

        for t in threads {
            t.join().unwrap();
        }

        let rem = remaining.load(SeqCst);
        assert!(rem > 0);
        assert_eq!(d.len(), rem);

        {
            let mut v = dropped.lock().unwrap();
            assert_eq!(v.len(), COUNT - rem);
            v.clear();
        }

        drop(d);

        {
            let mut v = dropped.lock().unwrap();
            assert_eq!(v.len(), rem);
            v.sort();
            for pair in v.windows(2) {
                assert_eq!(pair[0] + 1, pair[1]);
            }
        }
    }
}
