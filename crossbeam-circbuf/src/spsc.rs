//! Concurrent single-producer single-consumer queues based on circular buffer.

/// Bounded SPSC queue based on fixed-sized concurrent circular buffer.
///
/// # Examples
///
/// ```
/// use crossbeam_circbuf::spsc::bounded as spsc;
/// use std::thread;
///
/// let (tx, rx) = spsc::new::<char>(16);
///
/// tx.send('a').unwrap();
/// tx.send('b').unwrap();
/// tx.send('c').unwrap();
///
/// assert_eq!(rx.recv(), Some('a'));
/// drop(tx);
///
/// thread::spawn(move || {
///     assert_eq!(rx.recv(), Some('b'));
///     assert_eq!(rx.recv(), Some('c'));
/// }).join().unwrap();
/// ```
pub mod bounded {
    use crate::sp_inner;
    pub use crate::TryRecv;

    /// The sender of a bounded SPSC queue.
    #[derive(Debug)]
    pub struct Sender<T>(sp_inner::CircBuf<T>);

    /// The receiver of a bounded SPSC queue.
    #[derive(Debug)]
    pub struct Receiver<T>(sp_inner::Receiver<T>);

    unsafe impl<T> Send for Receiver<T> {}

    /// Creates a bounded SPSC queue with the specified capacity, and returns its sender and
    /// receiver.
    ///
    /// If the capacity is not a power of two, it will be rounded up to the next one.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::spsc::bounded as spsc;
    ///
    /// let (tx, rx) = spsc::new::<u32>(16);
    /// ```
    pub fn new<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
        let circbuf = sp_inner::CircBuf::new(cap);
        let receiver = circbuf.receiver();
        let sender = Sender { 0: circbuf };
        let receiver = Receiver { 0: receiver };
        (sender, receiver)
    }

    impl<T> Sender<T> {
        /// Attempts to send a value to the queue.
        ///
        /// Returns `Ok(())` if the value is successfully sent; or `Err(value)` if the buffer is
        /// full and we failed to send `value`.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spsc::bounded as spsc;
        ///
        /// let (tx, rx) = spsc::new::<u32>(16);
        /// tx.send(1).unwrap();
        /// tx.send(2).unwrap();
        /// ```
        pub fn send(&self, value: T) -> Result<(), T> {
            self.0.send(value)
        }
    }

    impl<T> Receiver<T> {
        /// Receives a value from the queue.
        ///
        /// Returns `Some(v)` if `v` is received; or `None` if the queue is empty.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spsc::bounded as spsc;
        ///
        /// let (tx, rx) = spsc::new::<u32>(16);
        /// tx.send(32).unwrap();
        /// assert_eq!(rx.recv(), Some(32));
        /// ```
        pub fn recv(&self) -> Option<T> {
            // I'm the only receiver because (1) `Sender` doesn't receive, and (2) `Receiver` is not
            // `Sync`.
            unsafe { self.0.recv_exclusive() }
        }
    }
}

/// Unbounded SPSC queue based on dynamically growable and shrinkable concurrent circular buffer.
///
/// # Examples
///
/// ```
/// use crossbeam_circbuf::spsc::unbounded as spsc;
/// use std::thread;
///
/// let (tx, rx) = spsc::new::<char>();
///
/// tx.send('a');
/// tx.send('b');
/// tx.send('c');
///
/// assert_eq!(rx.recv(), Some('a'));
/// drop(tx);
///
/// thread::spawn(move || {
///     assert_eq!(rx.recv(), Some('b'));
///     assert_eq!(rx.recv(), Some('c'));
/// }).join().unwrap();
/// ```
pub mod unbounded {
    use crate::sp_inner;
    pub use crate::TryRecv;

    /// The sender of an unbounded SPSC queue.
    #[derive(Debug)]
    pub struct Sender<T>(sp_inner::DynamicCircBuf<T>);

    /// The receiver of an unbounded SPSC queue.
    #[derive(Debug)]
    pub struct Receiver<T>(sp_inner::Receiver<T>);

    unsafe impl<T> Send for Receiver<T> {}

    /// Creates an unbounded SPSC queue, and returns its sender and receiver.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::spsc::unbounded as spsc;
    ///
    /// let (tx, rx) = spsc::new::<u32>();
    /// ```
    pub fn new<T>() -> (Sender<T>, Receiver<T>) {
        let circbuf = sp_inner::DynamicCircBuf::new();
        let receiver = circbuf.receiver();
        let sender = Sender { 0: circbuf };
        let receiver = Receiver { 0: receiver };
        (sender, receiver)
    }

    /// Creates an unbounded SPSC queue with the specified minimal capacity, and returns its sender and
    /// receiver.
    ///
    /// If the capacity is not a power of two, it will be rounded up to the next one.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_circbuf::spsc::unbounded as spsc;
    ///
    /// // The minimum capacity will be rounded up to 1024.
    /// let (tx, rx) = spsc::with_min_capacity::<u32>(1000);
    /// ```
    pub fn with_min_capacity<T>(min_cap: usize) -> (Sender<T>, Receiver<T>) {
        let circbuf = sp_inner::DynamicCircBuf::with_min_capacity(min_cap);
        let receiver = circbuf.receiver();
        let sender = Sender { 0: circbuf };
        let receiver = Receiver { 0: receiver };
        (sender, receiver)
    }

    impl<T> Sender<T> {
        /// Attempts to send a value to the queue.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spsc::unbounded as spsc;
        ///
        /// let (tx, rx) = spsc::new::<u32>();
        /// tx.send(1);
        /// tx.send(2);
        /// ```
        pub fn send(&self, value: T) {
            self.0.send(value)
        }
    }

    impl<T> Receiver<T> {
        /// Receives a value from the queue.
        ///
        /// Returns `Some(v)` if `v` is received; or `None` if the queue is empty.
        ///
        /// # Examples
        ///
        /// ```
        /// use crossbeam_circbuf::spsc::unbounded as spsc;
        ///
        /// let (tx, rx) = spsc::new::<u32>();
        /// tx.send(32);
        /// assert_eq!(rx.recv(), Some(32));
        /// ```
        pub fn recv(&self) -> Option<T> {
            // I'm the only receiver because (1) `Sender` doesn't receive, and (2) `Receiver` is not
            // `Sync`.
            unsafe { self.0.recv_exclusive() }
        }
    }
}
