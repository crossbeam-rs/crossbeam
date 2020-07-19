//! Concurrent single-producer queues based on circular buffer.
//!
//! [`CircBuf`] is a circular buffer, which is basically a fixed-sized array that has two ends: tx
//! and rx. A [`CircBuf`] can [`send`] values into the tx end and [`CircBuf::try_recv`] values from
//! the rx end. A [`CircBuf`] doesn't implement `Sync` so it cannot be shared among multiple
//! threads. However, it can create [`Receiver`]s, and those can be easily cloned, shared, and sent
//! to other threads. [`Receiver`]s can only [`Receiver::try_recv`] values from the rx end.
//!
//! Here's a visualization of a [`CircBuf`] of capacity 4, consisting of 2 values `a` and `b`.
//!
//! ```text
//!    ___
//!   | a | <- rx (CircBuf::try_recv, Receiver::try_recv)
//!   | b |
//!   |   | <- tx (CircBuf::send)
//!   |   |
//!    ¯¯¯
//! ```
//!
//! [`DynamicCircBuf`] is a dynamically growable and shrinkable circular buffer. Internally,
//! [`DynamicCircBuf`] has a [`CircBuf`] and resizes it when necessary.
//!
//!
//! # Usage: fair work-stealing schedulers
//!
//! This data structure can be used in fair work-stealing schedulers for multiple threads as
//! follows.
//!
//! Each thread owns a [`CircBuf`] (or [`DynamicCircBuf`]) and creates a [`Receiver`] that is shared
//! among all other threads (or creates one [`Receiver`] for each of the other threads).
//!
//! Each thread is executing a loop in which it attempts to [`CircBuf::try_recv`] some task from its
//! own [`CircBuf`] and perform it. If the buffer is empty, it attempts to [`Receiver::try_recv`]
//! work from other threads instead. When performing a task, a thread may produce more tasks by
//! [`send`]ing them to its buffer.
//!
//! It is worth noting that it is discouraged to use work-stealing deque for fair schedulers,
//! because its `pop()` may return a task that is just `push()`ed, effectively scheduling the same
//! work repeatedly.
//!
//! [`CircBuf`]: struct.CircBuf.html
//! [`DynamicCircBuf`]: struct.DynamicCircBuf.html
//! [`Receiver`]: struct.Receiver.html
//! [`send`]: struct.CircBuf.html#method.send
//! [`CircBuf::try_recv`]: struct.CircBuf.html#method.try_recv
//! [`Receiver::try_recv`]: struct.Receiver.html#method.try_recv

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::{self, ManuallyDrop};
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use epoch::{self, Atomic};
use utils::CachePadded;

use buffer::Buffer;
pub use TryRecv;

/// Internal data shared among a circular buffer and its receivers.
#[derive(Debug)]
struct Inner<T> {
    /// The rx index.
    rx: CachePadded<AtomicUsize>,

    /// The tx index.
    tx: CachePadded<AtomicUsize>,

    /// The underlying buffer.
    buffer: Atomic<Buffer<T>>,
}

impl<T> Inner<T> {
    /// Returns a new `Inner` with minimum capacity of `min_cap` rounded to the next power of two.
    fn new(cap: usize) -> Self {
        debug_assert_eq!(cap, cap.next_power_of_two());

        let buffer = Buffer::new(cap);

        Inner {
            rx: CachePadded::new(AtomicUsize::new(0)),
            tx: CachePadded::new(AtomicUsize::new(0)),
            buffer: buffer.into(),
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Loads rx, tx, and buffer.
        let rx = self.rx.load(Ordering::Relaxed);
        let tx = self.tx.load(Ordering::Relaxed);

        unsafe {
            let buffer = self.buffer.load(Ordering::Relaxed, epoch::unprotected());

            // Drops the values from rx to tx in the buffer.
            for i in 0..tx.wrapping_sub(rx) {
                let mut value = buffer.deref().read_unchecked(rx.wrapping_add(i));
                ManuallyDrop::drop(&mut value);
            }

            // Free the memory allocated by the buffer.
            drop(buffer.into_owned());
        }
    }
}

/// A fixed-sized concurrent circular buffer.
///
/// A circular buffer has two ends: rx and tx. Values can be [`send`]ed to the tx end and
/// [`CircBuf::try_recv`]ed from the rx end. The rx end is special in that receivers can also
/// receive from the rx end using [`Receiver::try_recv`] method.
///
/// # Receivers
///
/// [`CircBuf`] may create [`Receiver`]s that can be shared among multiple threads. [`Receiver`]s
/// can only [`Receiver::try_recv`] values from the rx end of the circular buffer.
///
/// # Capacity
///
/// The capacity of a buffer is fixed when created.
///
/// [`CircBuf`]: struct.CircBuf.html
/// [`Receiver`]: struct.Receiver.html
/// [`send`]: struct.CircBuf.html#method.send
/// [`CircBuf::try_recv`]: struct.CircBuf.html#method.try_recv
/// [`Receiver::try_recv`]: struct.Receiver.html#method.try_recv
pub struct CircBuf<T> {
    /// Internal data of the underlying circular buffer.
    inner: Arc<Inner<T>>,

    /// The lower bound of the rx end in the view of the sender.
    rx_lb: Cell<usize>,

    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for CircBuf<T> {}

impl<T> CircBuf<T> {
    /// Returns a new circular buffer with the specified capacity.
    ///
    /// If the capacity is not a power of two, it will be rounded up to the next one.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::CircBuf;
    ///
    /// // The capacity will be rounded up to 1024.
    /// let cb = CircBuf::<i32>::new(1000);
    /// ```
    pub fn new(cap: usize) -> CircBuf<T> {
        let power = cap.next_power_of_two();
        assert!(power >= cap, "capacity too large: {}", cap);

        Self {
            inner: Arc::new(Inner::new(power)),
            rx_lb: Cell::new(0),
            _marker: PhantomData,
        }
    }

    /// Sends a value to the tx end of the circular buffer.
    ///
    /// Returns `Ok(())` if the value is successfully sent; or `Err(value)` if the circular buffer
    /// is full and we failed to send `value`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::CircBuf;
    ///
    /// let cb = CircBuf::new(16);
    /// cb.send(1).unwrap();
    /// cb.send(2).unwrap();
    /// ```
    pub fn send(&self, value: T) -> Result<(), T> {
        self.send_inner(value).map_err(|(value, _, _)| value)
    }

    /// Receives a value from the rx end of the circular buffer.
    ///
    /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
    /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while attempting
    /// to receive data.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{CircBuf, TryRecv};
    ///
    /// let cb = CircBuf::new(16);
    /// cb.send(1).unwrap();
    /// cb.send(2).unwrap();
    ///
    /// // Attempt to receive a value.
    /// //
    /// // It should return `TryRecv::Data(v)` for a value `v`, or `Err(TryRecv::Retry)`.
    /// assert_ne!(cb.try_recv(), TryRecv::Empty);
    /// ```
    ///
    /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
    pub fn try_recv(&self) -> TryRecv<T> {
        self.try_recv_inner().map(|v| v.0)
    }

    /// Helper function for [`CircBuf::send`] and [`DynamicCircBuf::send`].
    ///
    /// returns `Ok(())` if the value is successfully sent; or `Err((value, tx, cap))` if the
    /// circular buffer is full and we failed to send `value`.
    ///
    /// [`CircBuf::send`]: struct.CircBuf.html#method.send
    /// [`DynamicCircBuf::send`]: struct.DynamicCircBuf.html#method.send
    #[inline]
    fn send_inner(&self, value: T) -> Result<(), (T, usize, usize)> {
        unsafe {
            // Loads rx, tx, and buffer. The buffer doesn't have to be epoch-protected because the
            // current thread (the worker) is the only one that grows and shrinks it.
            let buffer = self
                .inner
                .buffer
                .load(Ordering::Relaxed, epoch::unprotected());
            let tx = self.inner.tx.load(Ordering::Relaxed);
            let rx_lb = self.rx_lb.get();

            // Calculates the length and the capacity of the circular buffer.
            let len = tx.wrapping_sub(rx_lb) as isize;
            let cap = buffer.deref().len();

            // If the circular buffer is full, grows the underlying buffer.
            if len >= cap as isize {
                let rx = self.inner.rx.load(Ordering::Acquire);
                self.rx_lb.set(rx);
                let len = tx.wrapping_sub(rx) as isize;

                if len >= cap as isize {
                    return Err((value, tx, cap));
                }
            }

            // Writes `value` to the right slot and increments `tx`.
            buffer.deref().write(tx, value);
            self.inner.tx.store(tx.wrapping_add(1), Ordering::Release);
            Ok(())
        }
    }

    /// Helper function for [`CircBuf::try_recv`] and [`DynamicCircBuf::try_recv`].
    ///
    /// The return value is similar to [`CircBuf::try_recv`], but `TryRecv::Data((value,
    /// Some(cap)))` means `cap` is the current capacity of the buffer, and it's worth shrinking the
    /// buffer; and `TryRecv::Data((value, None))` means it's not worth shrinking the buffer.
    ///
    /// [`CircBuf::try_recv`]: struct.CircBuf.html#method.try_recv
    /// [`DynamicCircBuf::try_recv`]: struct.DynamicCircBuf.html#method.try_recv
    #[inline]
    fn try_recv_inner(&self) -> TryRecv<(T, Option<usize>)> {
        // Loads tx and rx.
        let tx = self.inner.tx.load(Ordering::Relaxed);
        let rx = self.inner.rx.load(Ordering::Acquire);
        let len = tx.wrapping_sub(rx) as isize;

        // Is the circular buffer empty?
        if len <= 0 {
            self.rx_lb.set(rx);
            return TryRecv::Empty;
        }

        // Tries incrementing rx to receive a value.
        let rx_new = rx.wrapping_add(1);
        if self
            .inner
            .rx
            .compare_exchange_weak(rx, rx_new, Ordering::Acquire, Ordering::Acquire)
            .map_err(|rx_cur| self.rx_lb.set(rx_cur))
            .is_err()
        {
            return TryRecv::Retry;
        }

        // Sets the lower bound of rx.
        self.rx_lb.set(rx_new);

        // Loads the value at the rx end of the buffer.
        unsafe {
            let buffer = self
                .inner
                .buffer
                .load(Ordering::Relaxed, epoch::unprotected());

            // Because len > 0, we should read a valid value.
            let value = buffer.deref().read_unchecked(rx);

            // Shrinks the buffer if `len - 1` is less than one fourth of `self.min_cap`.
            let cap = buffer.deref().len();
            let resize = if len <= cap as isize / 4 {
                Some(cap)
            } else {
                None
            };

            TryRecv::Data((ManuallyDrop::into_inner(value), resize))
        }
    }

    /// Creates a receiver that can be shared with other threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{CircBuf, TryRecv};
    /// use std::thread;
    ///
    /// let cb = CircBuf::new(16);
    /// cb.send(1).unwrap();
    /// cb.send(2).unwrap();
    ///
    /// let r = cb.receiver();
    ///
    /// thread::spawn(move || {
    ///     assert_eq!(r.try_recv(), TryRecv::Data(1));
    /// }).join().unwrap();
    /// ```
    pub fn receiver(&self) -> Receiver<T> {
        Receiver {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for CircBuf<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CircBuf {{ ... }}")
    }
}

/// A dynamic-sized concurrent circular buffer.
///
/// A circular buffer has two ends: rx and tx. Values can be [`send`]ed to the tx end and
/// [`DynamicCircBuf::try_recv`]ed from the rx end. The rx end is special in that receivers can also
/// receive from the rx end using [`Receiver::try_recv`] method.
///
/// # Receivers
///
/// [`CircBuf`] may create [`Receiver`]s that can be shared among multiple threads. [`Receiver`]s
/// can only [`Receiver::try_recv`] values from the rx end of the circular buffer.
///
/// # Capacity
///
/// A buffer grows and shrinks as values are inserted and removed from it. If the internal buffer
/// gets full, a new one twice the size of the original is allocated. Similarly, if it is less than
/// a quarter full, a new buffer half the size of the original is allocated.
///
/// In order to prevent frequent resizing (reallocations may be costly), it is possible to specify a
/// large minimum capacity for the circular buffer by calling
/// [`DynamicCircBuf::with_min_capacity`]. This constructor will make sure that the internal buffer
/// never shrinks below that size.
///
/// [`DynamicCircBuf`]: struct.DynamicCircBuf.html
/// [`Receiver`]: struct.Receiver.html
/// [`send`]: struct.DynamicCircBuf.html#method.send
/// [`DynamicCircBuf::with_min_capacity`]: struct.DynamicCircBuf.html#method.with_min_capacity
/// [`DynamicCircBuf::try_recv`]: struct.DynamicCircBuf.html#method.try_recv
/// [`Receiver::try_recv`]: struct.Receiver.html#method.try_recv
pub struct DynamicCircBuf<T> {
    /// The underlying circular buffer.
    inner: CircBuf<T>,

    /// Minimum capacity of the buffer. Always a power of two.
    min_cap: usize,

    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for DynamicCircBuf<T> {}

impl<T> DynamicCircBuf<T> {
    /// Minimum capacity for a dynamic circular buffer.
    const DEFAULT_MIN_CAP: usize = 1 << 4;

    /// If an buffer of at least this size is retired, thread-local garbage is flushed so that it
    /// gets deallocated as soon as possible.
    const FLUSH_THRESHOLD_BYTES: usize = 1 << 10;

    /// Returns a new circular buffer.
    ///
    /// The internal buffer is destructed as soon as the circular buffer and all its receivers get
    /// dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::DynamicCircBuf;
    ///
    /// let cb = DynamicCircBuf::<i32>::new();
    /// ```
    #[inline]
    pub fn new() -> DynamicCircBuf<T> {
        Self::with_min_capacity(Self::DEFAULT_MIN_CAP)
    }

    /// Returns a new circular buffer with the specified minimum capacity.
    ///
    /// If the capacity is not a power of two, it will be rounded up to the next one.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::DynamicCircBuf;
    ///
    /// // The minimum capacity will be rounded up to 1024.
    /// let cb = DynamicCircBuf::<i32>::with_min_capacity(1000);
    /// ```
    pub fn with_min_capacity(cap: usize) -> DynamicCircBuf<T> {
        let power = cap.next_power_of_two();
        assert!(power >= cap, "capacity too large: {}", cap);

        Self {
            inner: CircBuf::new(power),
            min_cap: power,
            _marker: PhantomData,
        }
    }

    /// Sends a value to the tx end of the circular buffer.
    ///
    /// If the internal buffer is full, a new one twice the capacity of the current one will be
    /// allocated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::DynamicCircBuf;
    ///
    /// let cb = DynamicCircBuf::new();
    /// cb.send(1);
    /// cb.send(2);
    /// ```
    pub fn send(&self, value: T) {
        self.inner
            .send_inner(value)
            .unwrap_or_else(|(value, tx, cap)| unsafe {
                // The circular buffer is full. Grows the buffer.
                self.resize(2 * cap);
                let buffer = self
                    .inner
                    .inner
                    .buffer
                    .load(Ordering::Relaxed, epoch::unprotected());

                // Writes `value` to the right slot and increment `tx`.
                buffer.deref().write(tx, value);
                self.inner
                    .inner
                    .tx
                    .store(tx.wrapping_add(1), Ordering::Release);
            })
    }

    /// Receives a value from the rx end of the circular buffer.
    ///
    /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
    /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while attempting
    /// to receive data.
    ///
    /// If the internal buffer is less than a quarter full, a new buffer half the capacity of the
    /// current one will be allocated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{DynamicCircBuf, TryRecv};
    ///
    /// let cb = DynamicCircBuf::new();
    /// cb.send(1);
    /// cb.send(2);
    ///
    /// // Attempt to receive a value.
    /// //
    /// // It should return `TryRecv::Data(v)` for a value `v`, or `Err(TryRecv::Retry)`.
    /// assert_ne!(cb.try_recv(), TryRecv::Empty);
    /// ```
    ///
    /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
    pub fn try_recv(&self) -> TryRecv<T> {
        self.inner.try_recv_inner().map(|(r, resize)| {
            // Shrinks the buffer if it's worth.
            if let Some(cap) = resize {
                if cap > self.min_cap {
                    unsafe {
                        self.resize(cap / 2);
                    }
                }
            }

            // Returns the received value.
            r
        })
    }

    /// Creates a receiver that can be shared with other threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{DynamicCircBuf, TryRecv};
    /// use std::thread;
    ///
    /// let cb = DynamicCircBuf::new();
    /// cb.send(1);
    /// cb.send(2);
    ///
    /// let r = cb.receiver();
    ///
    /// thread::spawn(move || {
    ///     assert_eq!(r.try_recv(), TryRecv::Data(1));
    /// }).join().unwrap();
    /// ```
    pub fn receiver(&self) -> Receiver<T> {
        self.inner.receiver()
    }

    /// Resizes the internal buffer to the new capacity of `new_cap`.
    #[cold]
    unsafe fn resize(&self, new_cap: usize) {
        let inner = &self.inner.inner;
        // Loads rx, tx, and buffer.
        let rx = inner.rx.load(Ordering::Relaxed);
        let tx = inner.tx.load(Ordering::Relaxed);
        let buffer = inner.buffer.load(Ordering::Relaxed, epoch::unprotected());

        // Allocates a new buffer.
        let new = Buffer::new(new_cap);

        // Copies data from the old buffer to the new one.
        let mut i = rx;
        while i != tx {
            ptr::copy_nonoverlapping(buffer.deref().get(i), new.get(i) as *const _ as *mut _, 1);
            i = i.wrapping_add(1);
        }

        // Stores the new buffer.
        inner.buffer.store(new, Ordering::Release);

        let guard = epoch::pin();

        // Destroys the old buffer later.
        guard.defer_destroy(buffer);

        // Flushes the thread-local garbage in order to deallocate it as soon as possible if the
        // buffer is very large.
        if mem::size_of::<T>() * new_cap >= Self::FLUSH_THRESHOLD_BYTES {
            guard.flush();
        }
    }
}

impl<T> Default for DynamicCircBuf<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for DynamicCircBuf<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DynamicCircBuf {{ ... }}")
    }
}

/// A receiver that receives values from the rx end of a circular buffer.
///
/// You can receive a value from the rx end using [`try_recv`]. If there are no concurrent receive
/// operation, you can receive a value from the rx end using [`recv_exclusive`].
///
/// Receivers implement `Clone`, `Send`, and `Sync` so that they can be shared among multiple
/// threads.
///
/// [`try_recv`]: struct.Receiver.html#method.try_recv
/// [`recv_exclusive`]: struct.Receiver.html#method.recv_exclusive
pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> Receiver<T> {
    /// Receives a value from the rx end of its circular buffer.
    ///
    /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
    /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while attempting
    /// to receive data.
    ///
    /// This method will not attempt to resize the internal buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{DynamicCircBuf, TryRecv};
    ///
    /// let cb = DynamicCircBuf::new();
    /// let r = cb.receiver();
    /// cb.send(1);
    /// cb.send(2);
    ///
    /// // Attempt to receive a value, but keep retrying if we get `Retry`.
    /// let stolen = loop {
    ///     match r.try_recv() {
    ///         TryRecv::Data(r) => break Some(r),
    ///         TryRecv::Empty => break None,
    ///         TryRecv::Retry => {}
    ///     }
    /// };
    /// assert_eq!(stolen, Some(1));
    /// ```
    ///
    /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
    pub fn try_recv(&self) -> TryRecv<T> {
        // Loads rx.
        let rx = self.inner.rx.load(Ordering::Relaxed);

        // Loads the value at the rx end of the buffer.
        let value = {
            let guard = epoch::pin();
            let buffer = self.inner.buffer.load_consume(&guard);
            match unsafe { buffer.deref().read(rx) } {
                None => return TryRecv::Empty,
                Some(value) => value,
            }
        };

        // Tries incrementing rx to receive the value.
        if self
            .inner
            .rx
            .compare_exchange(rx, rx.wrapping_add(1), Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            return TryRecv::Retry;
        }

        TryRecv::Data(ManuallyDrop::into_inner(value))
    }

    /// Receives half the values from the rx end of its circular buffer.
    ///
    /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
    /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while attempting
    /// to receive data.
    ///
    /// This method will not attempt to resize the internal buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{DynamicCircBuf, TryRecv};
    ///
    /// let cb = DynamicCircBuf::new();
    /// let r = cb.receiver();
    /// cb.send(1);
    /// cb.send(2);
    ///
    /// // Attempt to receive a value, but keep retrying if we get `Retry`.
    /// let stolen = loop {
    ///     match r.try_recv_half() {
    ///         TryRecv::Data(r) => break Some(r),
    ///         TryRecv::Empty => break None,
    ///         TryRecv::Retry => {}
    ///     }
    /// };
    /// assert_eq!(stolen, Some(vec![1]));
    /// ```
    ///
    /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
    pub fn try_recv_half(&self) -> TryRecv<Vec<T>> {
        // Loads rx and tx, and calculate the length of the buffer.
        let rx = self.inner.rx.load(Ordering::Relaxed);
        let tx = self.inner.tx.load(Ordering::Acquire);
        let len = tx.wrapping_sub(rx) as isize;

        if len <= 0 {
            return TryRecv::Empty;
        }

        // Calculates the number of values to receive.
        let num = ((len + 1) / 2) as usize;

        // Loads the values at [rx, rx + num).
        let values = {
            let guard = epoch::pin();
            let buffer = unsafe { self.inner.buffer.load_consume(&guard).deref() };
            (0..num)
                .map(|i| unsafe { buffer.read(rx.wrapping_add(i)) })
                .collect::<Option<Vec<ManuallyDrop<T>>>>()
        };

        let values = match values {
            None => {
                // The buffer is already wrapped around, because we already acknowledged the values
                // in the range [rx, tx) thanks to release/acquire synchronization via `tx`.
                return TryRecv::Retry;
            }
            Some(values) => values,
        };

        if values.is_empty() {
            return TryRecv::Empty;
        }

        // Tries incrementing rx to receive the values.
        if self
            .inner
            .rx
            .compare_exchange(
                rx,
                rx.wrapping_add(num),
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_err()
        {
            // We didn't receive the values.
            return TryRecv::Retry;
        }

        TryRecv::Data(values.into_iter().map(ManuallyDrop::into_inner).collect())
    }

    /// Receives a value from the rx end of its circular buffer.
    ///
    /// Returns `Some(v)` if a value `v` is received; or `None` if the circular buffer is empty.
    ///
    /// This method will not attempt to resize the internal buffer.
    ///
    /// # Safety
    ///
    /// You have to guarantee that there are no concurrent receive operations, such as
    /// [`CircBuf::try_recv`], [`DynamicCircBuf::try_recv`], [`Receiver::try_recv`], and
    /// [`recv_exclusive`]. In other words, other receive operations should happen either before or
    /// after it.
    ///
    /// This method will not attempt to resize the internal buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::sp_inner::{DynamicCircBuf, TryRecv};
    ///
    /// let cb = DynamicCircBuf::new();
    /// let r = cb.receiver();
    /// cb.send(1);
    /// cb.send(2);
    ///
    /// // Attempt to receive a value.
    /// let stolen = unsafe { r.recv_exclusive() };
    /// assert_eq!(stolen, Some(1));
    /// ```
    ///
    /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
    /// [`CircBuf::try_recv`]: struct.CircBuf.html#method.try_recv
    /// [`DynamicCircBuf::try_recv`]: struct.DynamicCircBuf.html#method.try_recv
    /// [`Receiver::try_recv`]: struct.Receiver.html#method.try_recv
    /// [`recv_exclusive`]: struct.Receiver.html#method.recv_exclusive
    pub unsafe fn recv_exclusive(&self) -> Option<T> {
        // Loads rx.
        let rx = self.inner.rx.load(Ordering::Relaxed);

        // Loads the value at the rx end of the buffer.
        let value = {
            let guard = epoch::pin();
            let buffer = self.inner.buffer.load_consume(&guard);
            match buffer.deref().read(rx) {
                None => return None,
                Some(value) => value,
            }
        };

        // Increments rx to receive the value.
        self.inner.rx.store(rx.wrapping_add(1), Ordering::Release);
        Some(ManuallyDrop::into_inner(value))
    }
}

impl<T> Clone for Receiver<T> {
    /// Creates another receiver.
    fn clone(&self) -> Receiver<T> {
        Receiver {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Receiver {{ ... }}")
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use std::sync::atomic::Ordering::SeqCst;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use self::rand::Rng;

    use epoch;

    use super::{DynamicCircBuf, TryRecv};

    fn retry<T, F: Fn() -> TryRecv<T>>(f: F) -> Option<T> {
        loop {
            match f() {
                TryRecv::Data(r) => return Some(r),
                TryRecv::Empty => return None,
                TryRecv::Retry => {}
            }
        }
    }

    #[test]
    fn smoke() {
        let cb = DynamicCircBuf::new();
        let r = cb.receiver();

        assert_eq!(retry(|| cb.try_recv()), None);
        assert_eq!(retry(|| r.try_recv()), None);

        cb.send(1);
        assert_eq!(retry(|| cb.try_recv()), Some(1));
        assert_eq!(retry(|| cb.try_recv()), None);
        assert_eq!(retry(|| r.try_recv()), None);

        cb.send(2);
        assert_eq!(retry(|| r.try_recv()), Some(2));
        assert_eq!(retry(|| r.try_recv()), None);
        assert_eq!(retry(|| cb.try_recv()), None);

        cb.send(3);
        cb.send(4);
        cb.send(5);
        assert_eq!(retry(|| cb.try_recv()), Some(3));
        assert_eq!(retry(|| r.try_recv()), Some(4));
        assert_eq!(retry(|| cb.try_recv()), Some(5));
        assert_eq!(retry(|| cb.try_recv()), None);
    }

    #[test]
    fn try_recv_send() {
        const STEPS: usize = 50_000;

        let cb = DynamicCircBuf::new();
        let r = cb.receiver();
        let t = thread::spawn(move || {
            for i in 0..STEPS {
                loop {
                    if let TryRecv::Data(v) = r.try_recv() {
                        assert_eq!(i, v);
                        break;
                    }
                }
            }
        });

        for i in 0..STEPS {
            cb.send(i);
        }
        t.join().unwrap();
    }

    #[test]
    fn try_recv_half_send() {
        const STEPS: usize = 50_000;

        let cb = DynamicCircBuf::new();
        let r = cb.receiver();
        let t = thread::spawn(move || {
            let mut i = 0;
            loop {
                if let TryRecv::Data(v) = r.try_recv_half() {
                    for j in v {
                        assert_eq!(i, j);
                        i += 1;
                    }

                    if i == STEPS {
                        break;
                    }
                }
            }
        });

        for i in 0..STEPS {
            cb.send(i);
        }
        t.join().unwrap();
    }

    #[test]
    fn stampede() {
        const COUNT: usize = 50_000;

        let cb = DynamicCircBuf::new();

        for i in 0..COUNT {
            cb.send(Box::new(i + 1));
        }
        let remaining = Arc::new(AtomicUsize::new(COUNT));

        let threads = (0..8)
            .map(|_| {
                let r = cb.receiver();
                let remaining = remaining.clone();

                thread::spawn(move || {
                    let mut last = 0;
                    while remaining.load(SeqCst) > 0 {
                        if let TryRecv::Data(x) = r.try_recv() {
                            assert!(last < *x);
                            last = *x;
                            remaining.fetch_sub(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        while remaining.load(SeqCst) > 0 {
            if let TryRecv::Data(_) = cb.try_recv() {
                remaining.fetch_sub(1, SeqCst);
            }
        }

        for t in threads {
            t.join().unwrap();
        }
    }

    fn run_stress() {
        const COUNT: usize = 50_000;

        let cb = DynamicCircBuf::new();
        let done = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));

        let threads = (0..8)
            .map(|_| {
                let r = cb.receiver();
                let done = done.clone();
                let hits = hits.clone();

                thread::spawn(move || {
                    while !done.load(SeqCst) {
                        if let TryRecv::Data(_) = r.try_recv() {
                            hits.fetch_add(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut rng = rand::thread_rng();
        let mut expected = 0;
        while expected < COUNT {
            if rng.gen_range(0, 3) == 0 {
                if let TryRecv::Data(_) = cb.try_recv() {
                    hits.fetch_add(1, SeqCst);
                }
            } else {
                cb.send(expected);
                expected += 1;
            }
        }

        while hits.load(SeqCst) < COUNT {
            if let TryRecv::Data(_) = cb.try_recv() {
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

        let cb = DynamicCircBuf::new();
        let done = Arc::new(AtomicBool::new(false));

        let (threads, hits): (Vec<_>, Vec<_>) = (0..8)
            .map(|_| {
                let r = cb.receiver();
                let done = done.clone();
                let hits = Arc::new(AtomicUsize::new(0));

                let t = {
                    let hits = hits.clone();
                    thread::spawn(move || {
                        while !done.load(SeqCst) {
                            if let TryRecv::Data(_) = r.try_recv() {
                                hits.fetch_add(1, SeqCst);
                            }
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
                    if let TryRecv::Data(_) = cb.try_recv() {
                        my_hits += 1;
                    }
                } else {
                    cb.send(i);
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

        let cb = DynamicCircBuf::new();

        let dropped = Arc::new(Mutex::new(Vec::new()));
        let remaining = Arc::new(AtomicUsize::new(COUNT));
        for i in 0..COUNT {
            cb.send(Elem(i, dropped.clone()));
        }

        let threads = (0..8)
            .map(|_| {
                let r = cb.receiver();
                let remaining = remaining.clone();

                thread::spawn(move || {
                    for _ in 0..1000 {
                        if let TryRecv::Data(_) = r.try_recv() {
                            remaining.fetch_sub(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        for _ in 0..1000 {
            if let TryRecv::Data(_) = cb.try_recv() {
                remaining.fetch_sub(1, SeqCst);
            }
        }

        for t in threads {
            t.join().unwrap();
        }

        let rem = remaining.load(SeqCst);
        assert!(rem > 0);

        {
            let mut v = dropped.lock().unwrap();
            assert_eq!(v.len(), COUNT - rem);
            v.clear();
        }

        drop(cb);

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
