//! The channel interface.

use std::fmt;
use std::isize;
use std::iter::FusedIterator;
use std::mem;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use context::Context;
use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use flavors;
use select::{Operation, SelectHandle, Token};

/// A channel in the form of one of the different flavors.
pub struct Channel<T> {
    /// The number of senders associated with this channel.
    senders: AtomicUsize,

    /// The number of receivers associated with this channel.
    receivers: AtomicUsize,

    /// This channel's flavor.
    flavor: ChannelFlavor<T>,
}

/// Channel flavors.
enum ChannelFlavor<T> {
    /// Bounded channel based on a preallocated array.
    Array(flavors::array::Channel<T>),

    /// Unbounded channel implemented as a linked list.
    List(flavors::list::Channel<T>),

    /// Zero-capacity channel.
    Zero(flavors::zero::Channel<T>),
}

/// Creates a channel of unbounded capacity.
///
/// This type of channel can hold any number of messages (i.e. it has infinite capacity).
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::unbounded();
///
/// // An expensive computation.
/// fn fib(n: i32) -> i32 {
///     if n <= 1 {
///         n
///     } else {
///         fib(n - 1) + fib(n - 2)
///     }
/// }
///
/// // Spawn a thread doing an expensive computation.
/// thread::spawn(move || {
///     s.send(fib(20));
/// });
///
/// // Let's see what's the result of the computation.
/// println!("{}", r.recv().unwrap());
/// ```
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        flavor: ChannelFlavor::List(flavors::list::Channel::new()),
    });

    let s = Sender::new(chan.clone());
    let r = Receiver::new(chan);
    (s, r)
}

/// Creates a channel of bounded capacity.
///
/// This type of channel has an internal buffer of length `cap` in which messages get queued.
///
/// A rather special case is zero-capacity channel, also known as *rendezvous* channel. Such a
/// channel cannot hold any messages since its buffer is of length zero. Instead, send and receive
/// operations must be executing at the same time in order to pair up and pass the message over.
///
/// # Panics
///
/// Panics if the capacity is greater than `usize::max_value() / 4`.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::bounded(1);
///
/// // This call returns immediately since there is enough space in the channel.
/// s.send(1);
///
/// thread::spawn(move || {
///     // This call blocks the current thread because the channel is full.
///     // It will be able to complete only after the first message is received.
///     s.send(2);
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Some(1));
/// assert_eq!(r.recv(), Some(2));
/// ```
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::bounded(0);
///
/// thread::spawn(move || {
///     // This call blocks the current thread until a receive operation appears
///     // on the other side of the channel.
///     s.send(1);
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Some(1));
/// ```
pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        receivers: AtomicUsize::new(0),
        flavor: {
            if cap == 0 {
                ChannelFlavor::Zero(flavors::zero::Channel::new())
            } else {
                ChannelFlavor::Array(flavors::array::Channel::with_capacity(cap))
            }
        },
    });

    let s = Sender::new(chan.clone());
    let r = Receiver::new(chan);
    (s, r)
}

/// Creates a receiver that delivers a message after a certain duration of time.
///
/// The channel is bounded with capacity of 1 and is never closed. Exactly one message will be
/// automatically sent into the channel after `duration` elapses. The message is the instant at
/// which it is sent into the channel.
///
/// # Examples
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::time::Duration;
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::unbounded::<i32>();
///
/// let timeout = Duration::from_millis(100);
/// select! {
///     recv(r, msg) => println!("got {:?}", msg),
///     recv(channel::after(timeout)) => println!("timed out"),
/// }
/// # }
/// ```
///
/// ```
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// // Converts a number into a `Duration` in milliseconds.
/// let ms = |ms| Duration::from_millis(ms);
///
/// // Returns `true` if `a` and `b` are very close `Instant`s.
/// let eq = |a, b| a + ms(50) > b && b + ms(50) > a;
///
/// let start = Instant::now();
/// let r = channel::after(ms(100));
///
/// thread::sleep(ms(500));
///
/// // This message was sent 100 ms from the start and received 500 ms from the start.
/// assert!(eq(r.recv().unwrap(), start + ms(100)));
/// assert!(eq(Instant::now(), start + ms(500)));
/// ```
pub fn after(duration: Duration) -> Receiver<Instant> {
    Receiver {
        flavor: ReceiverFlavor::After(flavors::after::Channel::new(duration)),
    }
}

/// Creates a receiver that delivers messages periodically.
///
/// The channel is bounded with capacity of 1 and is never closed. Messages will be automatically
/// sent into the channel in intervals of `duration`, but the time intervals are only measured
/// while the channel is empty. The channel always contains at most one message. Each message is
/// the instant at which it is sent into the channel.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// // Converts a number into a `Duration` in milliseconds.
/// let ms = |ms| Duration::from_millis(ms);
///
/// // Returns `true` if `a` and `b` are very close `Instant`s.
/// let eq = |a, b| a + ms(50) > b && b + ms(50) > a;
///
/// let start = Instant::now();
/// let r = channel::tick(ms(100));
///
/// // This message was sent 100 ms from the start and received 100 ms from the start.
/// assert!(eq(r.recv().unwrap(), start + ms(100)));
/// assert!(eq(Instant::now(), start + ms(100)));
///
/// thread::sleep(ms(500));
///
/// // This message was sent 200 ms from the start and received 600 ms from the start.
/// assert!(eq(r.recv().unwrap(), start + ms(200)));
/// assert!(eq(Instant::now(), start + ms(600)));
///
/// // This message was sent 700 ms from the start and received 700 ms from the start.
/// assert!(eq(r.recv().unwrap(), start + ms(700)));
/// assert!(eq(Instant::now(), start + ms(700)));
/// ```
pub fn tick(duration: Duration) -> Receiver<Instant> {
    Receiver {
        flavor: ReceiverFlavor::Tick(flavors::tick::Channel::new(duration)),
    }
}

/// The sending side of a channel.
///
/// Senders can be cloned and shared among multiple threads.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r) = channel::unbounded();
/// let s2 = s1.clone();
///
/// thread::spawn(move || s1.send(1));
/// thread::spawn(move || s2.send(2));
///
/// let msg1 = r.recv().unwrap();
/// let msg2 = r.recv().unwrap();
///
/// assert_eq!(msg1 + msg2, 3);
/// ```
pub struct Sender<T> {
    inner: Arc<Channel<T>>,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> UnwindSafe for Sender<T> {}
impl<T> RefUnwindSafe for Sender<T> {}

impl<T> Sender<T> {
    /// Creates a sender handle for the channel and increments the sender count.
    fn new(chan: Arc<Channel<T>>) -> Self {
        let old_count = chan.senders.fetch_add(1, Ordering::SeqCst);

        // Cloning senders and calling `mem::forget` on the clones could potentially overflow the
        // counter. It's very difficult to recover sensibly from such degenerate scenarios so we
        // just abort when the count becomes very large.
        if old_count > isize::MAX as usize {
            process::abort();
        }

        Sender { inner: chan }
    }

    /// Attempts to send a message into the channel without blocking.
    ///
    /// This method will either send a message into the channel immediately, or return an error if
    /// the channel is full or disconnected. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will send a message the message only if
    /// there happens to be a receive operation on the other side of the channel at the same time.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, TrySendError};
    ///
    /// let (tx, rx) = bounded(1);
    /// assert_eq!(tx.try_send(1), Ok(()));
    /// assert_eq!(tx.try_send(2), Err(TrySendError::Full(2)));
    /// drop(rx);
    /// assert_eq!(tx.try_send(2), Err(TrySendError::Disconnected(2)));
    /// ```
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.try_send(msg),
            ChannelFlavor::List(chan) => chan.try_send(msg),
            ChannelFlavor::Zero(chan) => chan.try_send(msg),
        }
    }

    /// Sends a message into the channel, blocking the current thread if the channel is full.
    ///
    /// If called on a zero-capacity channel, this method blocks the current thread until a receive
    /// operation appears on the other side of the channel.
    ///
    /// Note: `s.send(msg)` is equivalent to `select! { send(s, msg) => {} }`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::bounded(0);
    ///
    /// thread::spawn(move || s.send(1));
    ///
    /// assert_eq!(r.recv(), Some(1));
    /// ```
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.send(msg, None),
            ChannelFlavor::List(chan) => chan.send(msg, None),
            ChannelFlavor::Zero(chan) => chan.send(msg, None),
        }.map_err(|err| {
            match err {
                SendTimeoutError::Disconnected(msg) => SendError(msg),
                SendTimeoutError::Timeout(_) => unreachable!(),
            }
        })
    }

    /// Sends a message into the channel, blocking if the channel is full for a limited time.
    ///
    /// If the channel is full (its capacity is fully utilized), this call will block until the
    /// send operation can proceed. If the channel is (or becomes) disconnected, or if it waits for
    /// longer than `timeout`, this call will wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, RecvTimeoutError};
    ///
    /// let (tx, rx) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     tx.send(5).unwrap();
    ///     drop(tx);
    /// });
    ///
    /// assert_eq!(rx.recv_timeout(Duration::from_millis(500)), Err(RecvTimeoutError::Timeout));
    /// assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(5));
    /// assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Disconnected));
    /// ```
    pub fn send_timeout(&self, msg: T, timeout: Duration) -> Result<(), SendTimeoutError<T>> {
        let deadline = Instant::now() + timeout;

        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.send(msg, Some(deadline)),
            ChannelFlavor::List(chan) => chan.send(msg, Some(deadline)),
            ChannelFlavor::Zero(chan) => chan.send(msg, Some(deadline)),
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::unbounded();
    /// assert!(s.is_empty());
    ///
    /// s.send(0);
    /// assert!(!s.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.is_empty(),
            ChannelFlavor::List(chan) => chan.is_empty(),
            ChannelFlavor::Zero(chan) => chan.is_empty(),
        }
    }

    /// Returns `true` if the channel is full.
    ///
    /// Note: zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::bounded(1);
    ///
    /// assert!(!s.is_full());
    /// s.send(0);
    /// assert!(s.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.is_full(),
            ChannelFlavor::List(chan) => chan.is_full(),
            ChannelFlavor::Zero(chan) => chan.is_full(),
        }
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::unbounded();
    /// assert_eq!(s.len(), 0);
    ///
    /// s.send(1);
    /// s.send(2);
    /// assert_eq!(s.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.len(),
            ChannelFlavor::List(chan) => chan.len(),
            ChannelFlavor::Zero(chan) => chan.len(),
        }
    }

    /// If the channel is bounded, returns its capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, _) = channel::unbounded::<i32>();
    /// assert_eq!(s.capacity(), None);
    ///
    /// let (s, _) = channel::bounded::<i32>(5);
    /// assert_eq!(s.capacity(), Some(5));
    ///
    /// let (s, _) = channel::bounded::<i32>(0);
    /// assert_eq!(s.capacity(), Some(0));
    /// ```
    pub fn capacity(&self) -> Option<usize> {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.capacity(),
            ChannelFlavor::List(chan) => chan.capacity(),
            ChannelFlavor::Zero(chan) => chan.capacity(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.inner.senders.fetch_sub(1, Ordering::SeqCst) == 1 {
            match &self.inner.flavor {
                ChannelFlavor::Array(chan) => chan.close(),
                ChannelFlavor::List(chan) => chan.close(),
                ChannelFlavor::Zero(chan) => chan.close(),
            };
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender::new(self.inner.clone())
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Sender").finish()
    }
}

/// The receiving side of a channel.
///
/// Receivers can be cloned and shared among multiple threads.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::unbounded();
///
/// thread::spawn(move || {
///     s.send("Hello world!");
///     thread::sleep(Duration::from_secs(2));
///     s.send("Delayed for 2 seconds");
/// });
///
/// println!("{}", r.recv().unwrap()); // Received immediately.
/// println!("Waiting...");
/// println!("{}", r.recv().unwrap()); // Received after 2 seconds.
/// ```
pub struct Receiver<T> {
    flavor: ReceiverFlavor<T>
}

/// Receiver flavors.
pub enum ReceiverFlavor<T> {
    /// A regular channel (array, list, or zero flavor).
    Channel(Arc<Channel<T>>),

    /// The after flavor.
    After(flavors::after::Channel),

    /// The tick flavor.
    Tick(flavors::tick::Channel),
}

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> UnwindSafe for Receiver<T> {}
impl<T> RefUnwindSafe for Receiver<T> {}

impl<T> Receiver<T> {
    /// Creates a receiver handle for the channel and increments the receiver count.
    fn new(chan: Arc<Channel<T>>) -> Self {
        let old_count = chan.receivers.fetch_add(1, Ordering::SeqCst);

        // Cloning receivers and calling `mem::forget` on the clones could potentially overflow the
        // counter. It's very difficult to recover sensibly from such degenerate scenarios so we
        // just abort when the count becomes very large.
        if old_count > isize::MAX as usize {
            process::abort(); // TODO: use utils::abort?
        }

        Receiver {
            flavor: ReceiverFlavor::Channel(chan),
        }
    }

    /// Attempts to receive a message from the channel without blocking.
    ///
    /// If there is no message ready to be received, returns `None`.
    ///
    /// Note: `r.try_recv()` is equivalent to `select! { recv(r, msg) => msg, default => None }`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::unbounded();
    /// assert_eq!(r.try_recv(), None);
    ///
    /// s.send(5);
    /// drop(s);
    ///
    /// assert_eq!(r.try_recv(), Some(5));
    /// assert_eq!(r.try_recv(), None);
    /// ```
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.try_recv(),
                ChannelFlavor::List(chan) => chan.try_recv(),
                ChannelFlavor::Zero(chan) => chan.try_recv(),
            },
            ReceiverFlavor::After(chan) => {
                let msg = chan.try_recv();
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, TryRecvError>,
                        Result<T, TryRecvError>
                    >(&msg)
                }
            }
            ReceiverFlavor::Tick(chan) => {
                let msg = chan.try_recv();
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, TryRecvError>,
                        Result<T, TryRecvError>
                    >(&msg)
                }
            }
        }
    }

    /// Blocks the current thread until a message is received or the channel is closed.
    ///
    /// Returns the message if it was received, or `None` if the channel is closed and empty.
    ///
    /// If called on a zero-capacity channel, this method blocks the current thread until a send
    /// operation appears on the other side of the channel or it becomes closed.
    ///
    /// Note: `r.recv()` is equivalent to `select! { recv(r, msg) => msg }`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5);
    ///     // `s` gets dropped, thus closing the channel.
    /// });
    ///
    /// assert_eq!(r.recv(), Some(5));
    /// assert_eq!(r.recv(), None);
    /// ```
    pub fn recv(&self) -> Result<T, RecvError> {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.recv(None),
                ChannelFlavor::List(chan) => chan.recv(None),
                ChannelFlavor::Zero(chan) => chan.recv(None),
            },
            ReceiverFlavor::After(chan) => {
                let msg = chan.recv(None);
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, RecvTimeoutError>,
                        Result<T, RecvTimeoutError>,
                    >(&msg)
                }
            },
            ReceiverFlavor::Tick(chan) => {
                let msg = chan.recv(None);
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, RecvTimeoutError>,
                        Result<T, RecvTimeoutError>,
                    >(&msg)
                }
            },
        }.map_err(|_| RecvError)
    }

    /// Waits for a message to be received from the channel but only for a limited time.
    ///
    /// This method will always block in order to wait for a message to become available. If the
    /// channel is (or becomes) empty and disconnected, or if it waits for longer than `timeout`,
    /// it will wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a send operation to appear
    /// on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, RecvTimeoutError};
    ///
    /// let (tx, rx) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     tx.send(5).unwrap();
    ///     drop(tx);
    /// });
    ///
    /// assert_eq!(rx.recv_timeout(Duration::from_millis(500)), Err(RecvTimeoutError::Timeout));
    /// assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok(5));
    /// assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Disconnected));
    /// ```
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;

        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.recv(Some(deadline)),
                ChannelFlavor::List(chan) => chan.recv(Some(deadline)),
                ChannelFlavor::Zero(chan) => chan.recv(Some(deadline)),
            },
            ReceiverFlavor::After(chan) => {
                let msg = chan.recv(Some(deadline));
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, RecvTimeoutError>,
                        Result<T, RecvTimeoutError>,
                    >(&msg)
                }
            },
            ReceiverFlavor::Tick(chan) => {
                let msg = chan.recv(Some(deadline));
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, RecvTimeoutError>,
                        Result<T, RecvTimeoutError>,
                    >(&msg)
                }
            },
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::unbounded();
    ///
    /// assert!(r.is_empty());
    /// s.send(0);
    /// assert!(!r.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.is_empty(),
                ChannelFlavor::List(chan) => chan.is_empty(),
                ChannelFlavor::Zero(chan) => chan.is_empty(),
            },
            ReceiverFlavor::After(chan) => chan.is_empty(),
            ReceiverFlavor::Tick(chan) => chan.is_empty(),
        }
    }

    /// Returns `true` if the channel is full.
    ///
    /// Note: zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::bounded(1);
    ///
    /// assert!(!r.is_full());
    /// s.send(0);
    /// assert!(r.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.is_full(),
                ChannelFlavor::List(chan) => chan.is_full(),
                ChannelFlavor::Zero(chan) => chan.is_full(),
            },
            ReceiverFlavor::After(chan) => !chan.is_empty(),
            ReceiverFlavor::Tick(chan) => !chan.is_empty(),
        }
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (s, r) = channel::unbounded();
    /// assert_eq!(r.len(), 0);
    ///
    /// s.send(1);
    /// s.send(2);
    /// assert_eq!(r.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.len(),
                ChannelFlavor::List(chan) => chan.len(),
                ChannelFlavor::Zero(chan) => chan.len(),
            },
            ReceiverFlavor::After(chan) => chan.len(),
            ReceiverFlavor::Tick(chan) => chan.len(),
        }
    }

    /// If the channel is bounded, returns its capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel as channel;
    ///
    /// let (_, r) = channel::unbounded::<i32>();
    /// assert_eq!(r.capacity(), None);
    ///
    /// let (_, r) = channel::bounded::<i32>(5);
    /// assert_eq!(r.capacity(), Some(5));
    ///
    /// let (_, r) = channel::bounded::<i32>(0);
    /// assert_eq!(r.capacity(), Some(0));
    /// ```
    pub fn capacity(&self) -> Option<usize> {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.capacity(),
                ChannelFlavor::List(chan) => chan.capacity(),
                ChannelFlavor::Zero(chan) => chan.capacity(),
            },
            ReceiverFlavor::After(chan) => chan.capacity(),
            ReceiverFlavor::Tick(chan) => chan.capacity(),
        }
    }

    /// Returns an iterator that waits for messages until the channel is disconnected.
    ///
    /// Each call to `next` will block waiting for the next message. It will finally return `None`
    /// when the channel is empty and disconnected.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded::<i32>();
    ///
    /// thread::spawn(move || {
    ///     tx.send(1).unwrap();
    ///     tx.send(2).unwrap();
    ///     tx.send(3).unwrap();
    /// });
    ///
    /// let v: Vec<_> = rx.iter().collect();
    /// assert_eq!(v, [1, 2, 3]);
    /// ```
    pub fn iter(&self) -> Iter<T> {
        Iter { rx: self }
    }

    /// Returns an iterator that receives messages until the channel is empty or disconnected.
    ///
    /// Each call to `next` will return a message if there is at least one in the channel. The
    /// iterator will never block waiting for new messages.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded::<i32>();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     tx.send(1).unwrap();
    ///     tx.send(2).unwrap();
    ///     thread::sleep(Duration::from_secs(2));
    ///     tx.send(3).unwrap();
    /// });
    ///
    /// thread::sleep(Duration::from_secs(2));
    /// let v: Vec<_> = rx.try_iter().collect();
    /// assert_eq!(v, [1, 2]);
    /// ```
    pub fn try_iter(&self) -> TryIter<T> {
        TryIter { rx: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let ReceiverFlavor::Channel(chan) = &self.flavor {
            if chan.receivers.fetch_sub(1, Ordering::SeqCst) == 1 {
                match &chan.flavor {
                    ChannelFlavor::Array(chan) => chan.close(),
                    ChannelFlavor::List(chan) => chan.close(),
                    ChannelFlavor::Zero(chan) => chan.close(),
                }
            }
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => Receiver::new(arc.clone()),
            ReceiverFlavor::After(chan) => Receiver {
                flavor: ReceiverFlavor::After(chan.clone()),
            },
            ReceiverFlavor::Tick(chan) => Receiver {
                flavor: ReceiverFlavor::Tick(chan.clone()),
            },
        }
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Receiver").finish()
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { rx: self }
    }
}

/// An iterator that waits for messages until the channel is disconnected.
///
/// Each call to `next` will block waiting for the next message. It will finally return `None` when
/// the channel is empty and disconnected.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (tx, rx) = unbounded();
///
/// thread::spawn(move || {
///     tx.send(1).unwrap();
///     tx.send(2).unwrap();
///     tx.send(3).unwrap();
/// });
///
/// let v: Vec<_> = rx.iter().collect();
/// assert_eq!(v, [1, 2, 3]);
/// ```
#[derive(Debug)]
pub struct Iter<'a, T: 'a> {
    rx: &'a Receiver<T>,
}

impl<'a, T> FusedIterator for Iter<'a, T> {}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}

/// An iterator that receives messages until the channel is empty or disconnected.
///
/// Each call to `next` will return a message if there is at least one in the channel. The iterator
/// will never block waiting for new messages.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::unbounded;
///
/// let (tx, rx) = unbounded::<i32>();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     tx.send(1).unwrap();
///     tx.send(2).unwrap();
///     thread::sleep(Duration::from_secs(2));
///     tx.send(3).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(2));
/// let v: Vec<_> = rx.try_iter().collect();
/// assert_eq!(v, [1, 2]);
/// ```
#[derive(Debug)]
pub struct TryIter<'a, T: 'a> {
    rx: &'a Receiver<T>,
}

impl<'a, T> FusedIterator for TryIter<'a, T> {}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.try_recv().ok()
    }
}

/// An owning iterator that waits for messages until the channel is disconnected.
///
/// Each call to `next` will block waiting for the next message. It will finally return `None` when
/// the channel is empty and disconnected.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (tx, rx) = unbounded();
///
/// thread::spawn(move || {
///     tx.send(1).unwrap();
///     tx.send(2).unwrap();
///     tx.send(3).unwrap();
/// });
///
/// let v: Vec<_> = rx.into_iter().collect();
/// assert_eq!(v, [1, 2, 3]);
/// ```
#[derive(Debug)]
pub struct IntoIter<T> {
    rx: Receiver<T>,
}

impl<T> FusedIterator for IntoIter<T> {}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}

impl<T> SelectHandle for Sender<T> {
    fn try(&self, token: &mut Token) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().try(token),
            ChannelFlavor::List(chan) => chan.sender().try(token),
            ChannelFlavor::Zero(chan) => chan.sender().try(token),
        }
    }

    fn retry(&self, token: &mut Token) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().retry(token),
            ChannelFlavor::List(chan) => chan.sender().retry(token),
            ChannelFlavor::Zero(chan) => chan.sender().retry(token),
        }
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, token: &mut Token, oper: Operation, cx: &Context) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().register(token, oper, cx),
            ChannelFlavor::List(chan) => chan.sender().register(token, oper, cx),
            ChannelFlavor::Zero(chan) => chan.sender().register(token, oper, cx),
        }
    }

    fn unregister(&self, oper: Operation) {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().unregister(oper),
            ChannelFlavor::List(chan) => chan.sender().unregister(oper),
            ChannelFlavor::Zero(chan) => chan.sender().unregister(oper),
        }
    }

    fn accept(&self, token: &mut Token, cx: &Context) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().accept(token, cx),
            ChannelFlavor::List(chan) => chan.sender().accept(token, cx),
            ChannelFlavor::Zero(chan) => chan.sender().accept(token, cx),
        }
    }

    fn state(&self) -> usize {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().state(),
            ChannelFlavor::List(chan) => chan.sender().state(),
            ChannelFlavor::Zero(chan) => chan.sender().state(),
        }
    }
}

impl<T> SelectHandle for Receiver<T> {
    fn try(&self, token: &mut Token) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().try(token),
                ChannelFlavor::List(chan) => chan.receiver().try(token),
                ChannelFlavor::Zero(chan) => chan.receiver().try(token),
            },
            ReceiverFlavor::After(chan) => chan.try(token),
            ReceiverFlavor::Tick(chan) => chan.try(token),
        }
    }

    fn retry(&self, token: &mut Token) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().retry(token),
                ChannelFlavor::List(chan) => chan.receiver().retry(token),
                ChannelFlavor::Zero(chan) => chan.receiver().retry(token),
            },
            ReceiverFlavor::After(chan) => chan.retry(token),
            ReceiverFlavor::Tick(chan) => chan.retry(token),
        }
    }

    fn deadline(&self) -> Option<Instant> {
        match &self.flavor {
            ReceiverFlavor::Channel(_) => None,
            ReceiverFlavor::After(chan) => chan.deadline(),
            ReceiverFlavor::Tick(chan) => chan.deadline(),
        }
    }

    fn register(&self, token: &mut Token, oper: Operation, cx: &Context) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().register(token, oper, cx),
                ChannelFlavor::List(chan) => chan.receiver().register(token, oper, cx),
                ChannelFlavor::Zero(chan) => chan.receiver().register(token, oper, cx),
            },
            ReceiverFlavor::After(chan) => chan.register(token, oper, cx),
            ReceiverFlavor::Tick(chan) => chan.register(token, oper, cx),
        }
    }

    fn unregister(&self, oper: Operation) {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().unregister(oper),
                ChannelFlavor::List(chan) => chan.receiver().unregister(oper),
                ChannelFlavor::Zero(chan) => chan.receiver().unregister(oper),
            },
            ReceiverFlavor::After(chan) => chan.unregister(oper),
            ReceiverFlavor::Tick(chan) => chan.unregister(oper),
        }
    }

    fn accept(&self, token: &mut Token, cx: &Context) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().accept(token, cx),
                ChannelFlavor::List(chan) => chan.receiver().accept(token, cx),
                ChannelFlavor::Zero(chan) => chan.receiver().accept(token, cx),
            },
            ReceiverFlavor::After(chan) => chan.accept(token, cx),
            ReceiverFlavor::Tick(chan) => chan.accept(token, cx),
        }
    }

    fn state(&self) -> usize {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().state(),
                ChannelFlavor::List(chan) => chan.receiver().state(),
                ChannelFlavor::Zero(chan) => chan.receiver().state(),
            },
            ReceiverFlavor::After(chan) => chan.state(),
            ReceiverFlavor::Tick(chan) => chan.state(),
        }
    }
}

/// Writes a message into the channel.
pub unsafe fn write<T>(s: &Sender<T>, token: &mut Token, msg: T) -> Result<(), T> {
    match &s.inner.flavor {
        ChannelFlavor::Array(chan) => chan.write(token, msg),
        ChannelFlavor::List(chan) => chan.write(token, msg),
        ChannelFlavor::Zero(chan) => chan.write(token, msg),
    }
}

/// Receives a message from the channel.
pub unsafe fn read<T>(r: &Receiver<T>, token: &mut Token) -> Result<T, ()> {
    match &r.flavor {
        ReceiverFlavor::Channel(arc) => match &arc.flavor {
            ChannelFlavor::Array(chan) => chan.read(token),
            ChannelFlavor::List(chan) => chan.read(token),
            ChannelFlavor::Zero(chan) => chan.read(token),
        },
        ReceiverFlavor::After(chan) => {
            mem::transmute_copy::<Result<Instant, ()>, Result<T, ()>>(&chan.read(token))
        }
        ReceiverFlavor::Tick(chan) => {
            mem::transmute_copy::<Result<Instant, ()>, Result<T, ()>>(&chan.read(token))
        }
    }
}
