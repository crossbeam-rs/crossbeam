//! Concurrent single-producer multiple-consumer queues based on circular buffer.

/// Bounded SPMC queue based on fixed-sized concurrent circular buffer.
///
/// # Examples
///
/// ```
/// use crossbeam_circbuf::TryRecv;
/// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
/// use std::thread;
///
/// let c = Queue::<char>::new(16);
/// let r = c.receiver();
///
/// c.send('a').unwrap();
/// c.send('b').unwrap();
/// c.send('c').unwrap();
///
/// assert_ne!(c.try_recv(), TryRecv::Empty); // TryRecv::Data('a') or TryRecv::Retry
/// drop(c);
///
/// thread::spawn(move || {
///     assert_ne!(r.try_recv(), TryRecv::Empty);
///     assert_ne!(r.try_recv(), TryRecv::Empty);
/// }).join().unwrap();
/// ```
pub mod bounded {
    use crate::{sp_inner, TryRecv};

    /// A bounded SPMC queue.
    #[derive(Debug)]
    pub struct Queue<T>(sp_inner::CircBuf<T>);

    /// The receiver of a bounded SPMC queue.
    #[derive(Debug)]
    pub struct Receiver<T>(sp_inner::Receiver<T>);

    impl<T> Queue<T> {
        /// Creates a bounded SPMC queue with the specified capacity.
        ///
        /// If the capacity is not a power of two, it will be rounded up to the next one.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new(16);
        /// ```
        pub fn new(cap: usize) -> Self {
            Queue {
                0: sp_inner::CircBuf::new(cap),
            }
        }

        /// Attempts to send a value to the queue.
        ///
        /// Returns `Ok(())` if the value is successfully sent; or `Err(value)` if the buffer is
        /// full and we failed to send `value`.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new(16);
        /// c.send(1).unwrap();
        /// c.send(2).unwrap();
        /// ```
        pub fn send(&self, value: T) -> Result<(), T> {
            self.0.send(value)
        }

        /// Receives a value from the queue.
        ///
        /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
        /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while
        /// attempting to receive data.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::TryRecv;
        /// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new(16);
        /// c.send(1).unwrap();
        /// c.send(2).unwrap();
        ///
        /// assert_ne!(c.try_recv(), TryRecv::Empty);
        /// assert_ne!(c.try_recv(), TryRecv::Empty);
        /// ```
        ///
        /// [`TryRecv::Data`]: enum.TryRecv.html#variant.Data
        /// [`TryRecv::Empty`]: enum.TryRecv.html#variant.Empty
        /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
        pub fn try_recv(&self) -> TryRecv<T> {
            self.0.try_recv()
        }

        /// Creates a receiver for the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new(16);
        /// let r = c.receiver();
        /// ```
        pub fn receiver(&self) -> Receiver<T> {
            Receiver {
                0: self.0.receiver(),
            }
        }
    }

    impl<T> Receiver<T> {
        /// Receives a value from the queue.
        ///
        /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
        /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while
        /// attempting to receive data.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::TryRecv;
        /// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new(16);
        /// c.send(1).unwrap();
        /// c.send(2).unwrap();
        ///
        /// let r = c.receiver();
        /// assert_ne!(r.try_recv(), TryRecv::Empty);
        /// assert_ne!(r.try_recv(), TryRecv::Empty);
        /// ```
        ///
        /// [`TryRecv::Data`]: enum.TryRecv.html#variant.Data
        /// [`TryRecv::Empty`]: enum.TryRecv.html#variant.Empty
        /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
        pub fn try_recv(&self) -> TryRecv<T> {
            self.0.try_recv()
        }

        /// Receives half the values from the queue.
        ///
        /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
        /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while
        /// attempting to receive data.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::TryRecv;
        /// use crossbeam_circbuf::spmc::bounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new(16);
        /// c.send(1).unwrap();
        /// c.send(2).unwrap();
        ///
        /// let r = c.receiver();
        /// assert_eq!(r.try_recv_half(), TryRecv::Data(vec![1]));
        /// assert_eq!(r.try_recv_half(), TryRecv::Data(vec![2]));
        /// ```
        ///
        /// [`TryRecv::Data`]: enum.TryRecv.html#variant.Data
        /// [`TryRecv::Empty`]: enum.TryRecv.html#variant.Empty
        /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
        pub fn try_recv_half(&self) -> TryRecv<Vec<T>> {
            self.0.try_recv_half()
        }
    }

    impl<T> Clone for Receiver<T> {
        /// Creates another receiver.
        fn clone(&self) -> Self {
            Self { 0: self.0.clone() }
        }
    }
}

/// Unbounded SPMC queue based on dynamically growable and shrinkable concurrent circular buffer.
///
/// # Examples
///
/// ```
/// use crossbeam_circbuf::TryRecv;
/// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
/// use std::thread;
///
/// let c = Queue::<char>::new();
/// let r = c.receiver();
///
/// c.send('a');
/// c.send('b');
/// c.send('c');
///
/// assert_ne!(c.try_recv(), TryRecv::Empty); // TryRecv::Data('a') or TryRecv::Retry
/// drop(c);
///
/// thread::spawn(move || {
///     assert_ne!(r.try_recv(), TryRecv::Empty);
///     assert_ne!(r.try_recv(), TryRecv::Empty);
/// }).join().unwrap();
/// ```
pub mod unbounded {
    use crate::sp_inner;
    use TryRecv;

    /// an unbounded SPMC queue.
    #[derive(Debug)]
    pub struct Queue<T>(sp_inner::DynamicCircBuf<T>);

    /// The receiver of an unbounded SPMC queue.
    #[derive(Debug)]
    pub struct Receiver<T>(sp_inner::Receiver<T>);

    impl<T> Queue<T> {
        /// Creates an unbounded SPMC queue.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new();
        /// ```
        pub fn new() -> Self {
            Queue {
                0: sp_inner::DynamicCircBuf::new(),
            }
        }

        /// Creates an unbounded SPMC queue with the specified minimal capacity.
        ///
        /// If the capacity is not a power of two, it will be rounded up to the next one.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// // The minimum capacity will be rounded up to 1024.
        /// let c = Queue::<u32>::with_min_capacity(1000);
        /// ```
        pub fn with_min_capacity(min_cap: usize) -> Self {
            Queue {
                0: sp_inner::DynamicCircBuf::with_min_capacity(min_cap),
            }
        }

        /// Attempts to send a value to the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new();
        /// c.send(1);
        /// c.send(2);
        /// ```
        pub fn send(&self, value: T) {
            self.0.send(value)
        }

        /// Receives a value from the queue.
        ///
        /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
        /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while
        /// attempting to receive data.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::TryRecv;
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new();
        /// c.send(1);
        /// c.send(2);
        ///
        /// assert_ne!(c.try_recv(), TryRecv::Empty);
        /// assert_ne!(c.try_recv(), TryRecv::Empty);
        /// ```
        ///
        /// [`TryRecv::Data`]: enum.TryRecv.html#variant.Data
        /// [`TryRecv::Empty`]: enum.TryRecv.html#variant.Empty
        /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
        pub fn try_recv(&self) -> TryRecv<T> {
            self.0.try_recv()
        }

        /// Creates a receiver for the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new();
        /// let r = c.receiver();
        /// ```
        pub fn receiver(&self) -> Receiver<T> {
            Receiver {
                0: self.0.receiver(),
            }
        }
    }

    impl<T> Default for Queue<T> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<T> Receiver<T> {
        /// Receives a value from the queue.
        ///
        /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
        /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while
        /// attempting to receive data.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::TryRecv;
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new();
        /// c.send(1);
        /// c.send(2);
        ///
        /// let r = c.receiver();
        /// assert_ne!(r.try_recv(), TryRecv::Empty);
        /// assert_ne!(r.try_recv(), TryRecv::Empty);
        /// ```
        ///
        /// [`TryRecv::Data`]: enum.TryRecv.html#variant.Data
        /// [`TryRecv::Empty`]: enum.TryRecv.html#variant.Empty
        /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
        pub fn try_recv(&self) -> TryRecv<T> {
            self.0.try_recv()
        }

        /// Receives half the values from the queue.
        ///
        /// Returns `TryRecv::Data(v)` if a value `v` is received; `TryRecv::Empty` if the circular
        /// buffer is empty; or [`TryRecv::Retry`] if another operation gets in the way while
        /// attempting to receive data.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::TryRecv;
        /// use crossbeam_circbuf::spmc::unbounded::{Queue, Receiver};
        ///
        /// let c = Queue::<u32>::new();
        /// c.send(1);
        /// c.send(2);
        ///
        /// let r = c.receiver();
        /// assert_eq!(r.try_recv_half(), TryRecv::Data(vec![1]));
        /// assert_eq!(r.try_recv_half(), TryRecv::Data(vec![2]));
        /// ```
        ///
        /// [`TryRecv::Data`]: enum.TryRecv.html#variant.Data
        /// [`TryRecv::Empty`]: enum.TryRecv.html#variant.Empty
        /// [`TryRecv::Retry`]: enum.TryRecv.html#variant.Retry
        pub fn try_recv_half(&self) -> TryRecv<Vec<T>> {
            self.0.try_recv_half()
        }
    }

    impl<T> Clone for Receiver<T> {
        /// Creates another receiver.
        fn clone(&self) -> Self {
            Self { 0: self.0.clone() }
        }
    }
}
