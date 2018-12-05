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

/// A channel in the form of one of the three different flavors.
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
/// This channel has a growable buffer that can hold any number of messages at a time.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (s, r) = unbounded();
///
/// // Computes the n-th Fibonacci number.
/// fn fib(n: i32) -> i32 {
///     if n <= 1 {
///         n
///     } else {
///         fib(n - 1) + fib(n - 2)
///     }
/// }
///
/// // Spawn an asynchronous computation.
/// thread::spawn(move || s.send(fib(20)).unwrap());
///
/// // Print the result of the computation.
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
/// This channel has a buffer that can hold at most `cap` messages at a time.
///
/// A special case is zero-capacity channel, which cannot hold any messages. Instead, send and
/// receive operations must appear at the same time in order to pair up and pass the message over.
///
/// # Panics
///
/// Panics if the capacity is greater than `usize::max_value() / 4`.
///
/// # Examples
///
/// A channel of capacity 1:
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::bounded;
///
/// let (s, r) = bounded(1);
///
/// // This call returns immediately because there is enough space in the channel.
/// s.send(1).unwrap();
///
/// thread::spawn(move || {
///     // This call blocks the current thread because the channel is full.
///     // It will be able to complete only after the first message is received.
///     s.send(2).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Ok(1));
/// assert_eq!(r.recv(), Ok(2));
/// ```
///
/// A zero-capacity channel:
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::bounded;
///
/// let (s, r) = bounded(0);
///
/// thread::spawn(move || {
///     // This call blocks the current thread until a receive operation appears
///     // on the other side of the channel.
///     s.send(1).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Ok(1));
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
/// The channel is bounded with capacity of 1 and never gets disconnected. Exactly one message will
/// be sent into the channel after `duration` elapses. The message is the instant at which it is
/// sent.
///
/// # Examples
///
/// Using an `after` channel for timeouts:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::time::Duration;
/// use crossbeam_channel::{after, unbounded};
///
/// let (s, r) = unbounded::<i32>();
/// let timeout = Duration::from_millis(100);
///
/// select! {
///     recv(r) -> msg => println!("received {:?}", msg),
///     recv(after(timeout)) -> _ => println!("timed out"),
/// }
/// # }
/// ```
///
/// When the message gets sent:
///
/// ```
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel::after;
///
/// // Converts a number of milliseconds into a `Duration`.
/// let ms = |ms| Duration::from_millis(ms);
///
/// // Returns `true` if `a` and `b` are very close `Instant`s.
/// let eq = |a, b| a + ms(50) > b && b + ms(50) > a;
///
/// let start = Instant::now();
/// let r = after(ms(100));
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

/// Creates a receiver that never delivers messages.
///
/// The channel is bounded with capacity of 0 and never gets disconnected.
///
/// # Examples
///
/// Using a `never` channel to optionally add a timeout to [`select!`]:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel::{after, never, unbounded};
///
/// let (s, r) = unbounded();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     s.send(1).unwrap();
/// });
///
/// // This duration can be a `Some` or a `None`.
/// let duration = Some(Duration::from_millis(100));
///
/// // Create a channel that times out after the specified duration.
/// let timeout = duration
///     .map(|d| after(d))
///     .unwrap_or(never());
///
/// select! {
///     recv(r) -> msg => assert_eq!(msg, Ok(1)),
///     recv(timeout) -> _ => println!("timed out"),
/// }
/// # }
/// ```
///
/// [`select!`]: macro.select.html
pub fn never<T>() -> Receiver<T> {
    Receiver {
        flavor: ReceiverFlavor::Never(flavors::never::Channel::new()),
    }
}

/// Creates a receiver that delivers messages periodically.
///
/// The channel is bounded with capacity of 1 and never gets disconnected. Messages will be
/// sent into the channel in intervals of `duration`. Each message is the instant at which it is
/// sent.
///
/// # Examples
///
/// Using a `tick` channel to periodically print elapsed time:
///
/// ```
/// use std::time::{Duration, Instant};
/// use crossbeam_channel::tick;
///
/// let start = Instant::now();
/// let ticker = tick(Duration::from_millis(100));
///
/// for _ in 0..5 {
///     ticker.recv().unwrap();
///     println!("elapsed: {:?}", start.elapsed());
/// }
/// ```
///
/// When messages get sent:
///
/// ```
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel::tick;
///
/// // Converts a number of milliseconds into a `Duration`.
/// let ms = |ms| Duration::from_millis(ms);
///
/// // Returns `true` if `a` and `b` are very close `Instant`s.
/// let eq = |a, b| a + ms(50) > b && b + ms(50) > a;
///
/// let start = Instant::now();
/// let r = tick(ms(100));
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
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (s1, r) = unbounded();
/// let s2 = s1.clone();
///
/// thread::spawn(move || s1.send(1).unwrap());
/// thread::spawn(move || s2.send(2).unwrap());
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
    /// This method will either send a message into the channel immediately or return an error if
    /// the channel is full or disconnected. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will send the message only if there
    /// happens to be a receive operation on the other side of the channel at the same time.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, TrySendError};
    ///
    /// let (s, r) = bounded(1);
    ///
    /// assert_eq!(s.try_send(1), Ok(()));
    /// assert_eq!(s.try_send(2), Err(TrySendError::Full(2)));
    ///
    /// drop(r);
    /// assert_eq!(s.try_send(3), Err(TrySendError::Disconnected(3)));
    /// ```
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.try_send(msg),
            ChannelFlavor::List(chan) => chan.try_send(msg),
            ChannelFlavor::Zero(chan) => chan.try_send(msg),
        }
    }

    /// Blocks the current thread until a message is sent or the channel is disconnected.
    ///
    /// If the channel is full and not disconnected, this call will block until the send operation
    /// can proceed. If the channel becomes disconnected, this call will wake up and return an
    /// error. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{bounded, SendError};
    ///
    /// let (s, r) = bounded(1);
    /// assert_eq!(s.send(1), Ok(()));
    ///
    /// thread::spawn(move || {
    ///     assert_eq!(r.recv(), Ok(1));
    ///     thread::sleep(Duration::from_secs(1));
    ///     drop(r);
    /// });
    ///
    /// assert_eq!(s.send(2), Ok(()));
    /// assert_eq!(s.send(3), Err(SendError(3)));
    /// ```
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.send(msg, None),
            ChannelFlavor::List(chan) => chan.send(msg, None),
            ChannelFlavor::Zero(chan) => chan.send(msg, None),
        }.map_err(|err| match err {
            SendTimeoutError::Disconnected(msg) => SendError(msg),
            SendTimeoutError::Timeout(_) => unreachable!(),
        })
    }

    /// Waits for a message to be sent into the channel, but only for a limited time.
    ///
    /// If the channel is full and not disconnected, this call will block until the send operation
    /// can proceed or the operation times out. If the channel becomes disconnected, this call will
    /// wake up and return an error. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{bounded, SendTimeoutError};
    ///
    /// let (s, r) = bounded(0);
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     assert_eq!(r.recv(), Ok(2));
    ///     drop(r);
    /// });
    ///
    /// assert_eq!(
    ///     s.send_timeout(1, Duration::from_millis(500)),
    ///     Err(SendTimeoutError::Timeout(1)),
    /// );
    /// assert_eq!(
    ///     s.send_timeout(2, Duration::from_secs(1)),
    ///     Ok(()),
    /// );
    /// assert_eq!(
    ///     s.send_timeout(3, Duration::from_millis(500)),
    ///     Err(SendTimeoutError::Disconnected(3)),
    /// );
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
    /// Note: Zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    /// assert!(s.is_empty());
    ///
    /// s.send(0).unwrap();
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
    /// Note: Zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::bounded;
    ///
    /// let (s, r) = bounded(1);
    ///
    /// assert!(!s.is_full());
    /// s.send(0).unwrap();
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
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    /// assert_eq!(s.len(), 0);
    ///
    /// s.send(1).unwrap();
    /// s.send(2).unwrap();
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
    /// use crossbeam_channel::{bounded, unbounded};
    ///
    /// let (s, _) = unbounded::<i32>();
    /// assert_eq!(s.capacity(), None);
    ///
    /// let (s, _) = bounded::<i32>(5);
    /// assert_eq!(s.capacity(), Some(5));
    ///
    /// let (s, _) = bounded::<i32>(0);
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
                ChannelFlavor::Array(chan) => chan.disconnect(),
                ChannelFlavor::List(chan) => chan.disconnect(),
                ChannelFlavor::Zero(chan) => chan.disconnect(),
            }
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
        f.pad("Sender { .. }")
    }
}

/// The receiving side of a channel.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::unbounded;
///
/// let (s, r) = unbounded();
///
/// thread::spawn(move || {
///     s.send(1);
///     thread::sleep(Duration::from_secs(1));
///     s.send(2);
/// });
///
/// assert_eq!(r.recv(), Ok(1)); // Received immediately.
/// assert_eq!(r.recv(), Ok(2)); // Received after 1 second.
/// ```
pub struct Receiver<T> {
    flavor: ReceiverFlavor<T>,
}

/// Receiver flavors.
pub enum ReceiverFlavor<T> {
    /// A regular channel (array, list, or zero flavor).
    Channel(Arc<Channel<T>>),

    /// The after flavor.
    After(flavors::after::Channel),

    /// The tick flavor.
    Tick(flavors::tick::Channel),

    /// The never flavor.
    Never(flavors::never::Channel<T>),
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
            process::abort();
        }

        Receiver {
            flavor: ReceiverFlavor::Channel(chan),
        }
    }

    /// Attempts to receive a message from the channel without blocking.
    ///
    /// This method will either receive a message from the channel immediately or return an error
    /// if the channel is empty.
    ///
    /// If called on a zero-capacity channel, this method will receive a message only if there
    /// happens to be a send operation on the other side of the channel at the same time.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, TryRecvError};
    ///
    /// let (s, r) = unbounded();
    /// assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
    ///
    /// s.send(5).unwrap();
    /// drop(s);
    ///
    /// assert_eq!(r.try_recv(), Ok(5));
    /// assert_eq!(r.try_recv(), Err(TryRecvError::Disconnected));
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
                    mem::transmute_copy::<Result<Instant, TryRecvError>, Result<T, TryRecvError>>(
                        &msg,
                    )
                }
            }
            ReceiverFlavor::Tick(chan) => {
                let msg = chan.try_recv();
                unsafe {
                    mem::transmute_copy::<Result<Instant, TryRecvError>, Result<T, TryRecvError>>(
                        &msg,
                    )
                }
            }
            ReceiverFlavor::Never(chan) => chan.try_recv(),
        }
    }

    /// Blocks the current thread until a message is received or the channel is empty and
    /// disconnected.
    ///
    /// If the channel is empty and not disconnected, this call will block until the receive
    /// operation can proceed. If the channel is empty and becomes disconnected, this call will
    /// wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a send operation to appear
    /// on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, RecvError};
    ///
    /// let (s, r) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5).unwrap();
    ///     drop(s);
    /// });
    ///
    /// assert_eq!(r.recv(), Ok(5));
    /// assert_eq!(r.recv(), Err(RecvError));
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
            }
            ReceiverFlavor::Tick(chan) => {
                let msg = chan.recv(None);
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, RecvTimeoutError>,
                        Result<T, RecvTimeoutError>,
                    >(&msg)
                }
            }
            ReceiverFlavor::Never(chan) => chan.recv(None),
        }.map_err(|_| RecvError)
    }

    /// Waits for a message to be received from the channel, but only for a limited time.
    ///
    /// If the channel is empty and not disconnected, this call will block until the receive
    /// operation can proceed or the operation times out. If the channel is empty and becomes
    /// disconnected, this call will wake up and return an error.
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
    /// let (s, r) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5).unwrap();
    ///     drop(s);
    /// });
    ///
    /// assert_eq!(
    ///     r.recv_timeout(Duration::from_millis(500)),
    ///     Err(RecvTimeoutError::Timeout),
    /// );
    /// assert_eq!(
    ///     r.recv_timeout(Duration::from_secs(1)),
    ///     Ok(5),
    /// );
    /// assert_eq!(
    ///     r.recv_timeout(Duration::from_secs(1)),
    ///     Err(RecvTimeoutError::Disconnected),
    /// );
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
            }
            ReceiverFlavor::Tick(chan) => {
                let msg = chan.recv(Some(deadline));
                unsafe {
                    mem::transmute_copy::<
                        Result<Instant, RecvTimeoutError>,
                        Result<T, RecvTimeoutError>,
                    >(&msg)
                }
            }
            ReceiverFlavor::Never(chan) => chan.recv(Some(deadline)),
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: Zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    ///
    /// assert!(r.is_empty());
    /// s.send(0).unwrap();
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
            ReceiverFlavor::Never(chan) => chan.is_empty(),
        }
    }

    /// Returns `true` if the channel is full.
    ///
    /// Note: Zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::bounded;
    ///
    /// let (s, r) = bounded(1);
    ///
    /// assert!(!r.is_full());
    /// s.send(0).unwrap();
    /// assert!(r.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.is_full(),
                ChannelFlavor::List(chan) => chan.is_full(),
                ChannelFlavor::Zero(chan) => chan.is_full(),
            },
            ReceiverFlavor::After(chan) => chan.is_full(),
            ReceiverFlavor::Tick(chan) => chan.is_full(),
            ReceiverFlavor::Never(chan) => chan.is_full(),
        }
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    /// assert_eq!(r.len(), 0);
    ///
    /// s.send(1).unwrap();
    /// s.send(2).unwrap();
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
            ReceiverFlavor::Never(chan) => chan.len(),
        }
    }

    /// If the channel is bounded, returns its capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, unbounded};
    ///
    /// let (_, r) = unbounded::<i32>();
    /// assert_eq!(r.capacity(), None);
    ///
    /// let (_, r) = bounded::<i32>(5);
    /// assert_eq!(r.capacity(), Some(5));
    ///
    /// let (_, r) = bounded::<i32>(0);
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
            ReceiverFlavor::Never(chan) => chan.capacity(),
        }
    }

    /// A blocking iterator over messages in the channel.
    ///
    /// Each call to [`next`] blocks waiting for the next message and then returns it. However, if
    /// the channel becomes empty and disconnected, it returns [`None`] without blocking.
    ///
    /// [`next`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.next
    /// [`None`]: https://doc.rust-lang.org/std/option/enum.Option.html#variant.None
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     s.send(1).unwrap();
    ///     s.send(2).unwrap();
    ///     s.send(3).unwrap();
    ///     drop(s); // Disconnect the channel.
    /// });
    ///
    /// // Collect all messages from the channel.
    /// // Note that the call to `collect` blocks until the sender is dropped.
    /// let v: Vec<_> = r.iter().collect();
    ///
    /// assert_eq!(v, [1, 2, 3]);
    /// ```
    pub fn iter(&self) -> Iter<T> {
        Iter { receiver: self }
    }

    /// A non-blocking iterator over messages in the channel.
    ///
    /// Each call to [`next`] returns a message if there is one ready to be received. The iterator
    /// never blocks waiting for the next message.
    ///
    /// [`next`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.next
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded::<i32>();
    ///
    /// thread::spawn(move || {
    ///     s.send(1).unwrap();
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(2).unwrap();
    ///     thread::sleep(Duration::from_secs(2));
    ///     s.send(3).unwrap();
    /// });
    ///
    /// thread::sleep(Duration::from_secs(2));
    ///
    /// // Collect all messages from the channel without blocking.
    /// // The third message hasn't been sent yet so we'll collect only the first two.
    /// let v: Vec<_> = r.try_iter().collect();
    ///
    /// assert_eq!(v, [1, 2]);
    /// ```
    pub fn try_iter(&self) -> TryIter<T> {
        TryIter { receiver: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let ReceiverFlavor::Channel(chan) = &self.flavor {
            if chan.receivers.fetch_sub(1, Ordering::SeqCst) == 1 {
                match &chan.flavor {
                    ChannelFlavor::Array(chan) => chan.disconnect(),
                    ChannelFlavor::List(chan) => chan.disconnect(),
                    ChannelFlavor::Zero(chan) => chan.disconnect(),
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
            ReceiverFlavor::Never(chan) => Receiver {
                flavor: ReceiverFlavor::Never(chan.clone()),
            },
        }
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Receiver { .. }")
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
        IntoIter { receiver: self }
    }
}

/// A blocking iterator over messages in a channel.
///
/// Each call to [`next`] blocks waiting for the next message and then returns it. However, if the
/// channel becomes empty and disconnected, it returns [`None`] without blocking.
///
/// [`next`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.next
/// [`None`]: https://doc.rust-lang.org/std/option/enum.Option.html#variant.None
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (s, r) = unbounded();
///
/// thread::spawn(move || {
///     s.send(1).unwrap();
///     s.send(2).unwrap();
///     s.send(3).unwrap();
///     drop(s); // Disconnect the channel.
/// });
///
/// // Collect all messages from the channel.
/// // Note that the call to `collect` blocks until the sender is dropped.
/// let v: Vec<_> = r.iter().collect();
///
/// assert_eq!(v, [1, 2, 3]);
/// ```
pub struct Iter<'a, T: 'a> {
    receiver: &'a Receiver<T>,
}

impl<'a, T> FusedIterator for Iter<'a, T> {}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

impl<'a, T> fmt::Debug for Iter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Iter { .. }")
    }
}

/// A non-blocking iterator over messages in a channel.
///
/// Each call to [`next`] returns a message if there is one ready to be received. The iterator
/// never blocks waiting for the next message.
///
/// [`next`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.next
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::unbounded;
///
/// let (s, r) = unbounded::<i32>();
///
/// thread::spawn(move || {
///     s.send(1).unwrap();
///     thread::sleep(Duration::from_secs(1));
///     s.send(2).unwrap();
///     thread::sleep(Duration::from_secs(2));
///     s.send(3).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(2));
///
/// // Collect all messages from the channel without blocking.
/// // The third message hasn't been sent yet so we'll collect only the first two.
/// let v: Vec<_> = r.try_iter().collect();
///
/// assert_eq!(v, [1, 2]);
/// ```
pub struct TryIter<'a, T: 'a> {
    receiver: &'a Receiver<T>,
}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.try_recv().ok()
    }
}

impl<'a, T> fmt::Debug for TryIter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("TryIter { .. }")
    }
}

/// A blocking iterator over messages in a channel.
///
/// Each call to [`next`] blocks waiting for the next message and then returns it. However, if the
/// channel becomes empty and disconnected, it returns [`None`] without blocking.
///
/// [`next`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.next
/// [`None`]: https://doc.rust-lang.org/std/option/enum.Option.html#variant.None
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (s, r) = unbounded();
///
/// thread::spawn(move || {
///     s.send(1).unwrap();
///     s.send(2).unwrap();
///     s.send(3).unwrap();
///     drop(s); // Disconnect the channel.
/// });
///
/// // Collect all messages from the channel.
/// // Note that the call to `collect` blocks until the sender is dropped.
/// let v: Vec<_> = r.into_iter().collect();
///
/// assert_eq!(v, [1, 2, 3]);
/// ```
pub struct IntoIter<T> {
    receiver: Receiver<T>,
}

impl<T> FusedIterator for IntoIter<T> {}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

impl<T> fmt::Debug for IntoIter<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("IntoIter { .. }")
    }
}

impl<T> SelectHandle for Sender<T> {
    fn try_select(&self, token: &mut Token) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().try_select(token),
            ChannelFlavor::List(chan) => chan.sender().try_select(token),
            ChannelFlavor::Zero(chan) => chan.sender().try_select(token),
        }
    }

    fn deadline(&self) -> Option<Instant> {
        None
    }

    fn register(&self, oper: Operation, cx: &Context) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().register(oper, cx),
            ChannelFlavor::List(chan) => chan.sender().register(oper, cx),
            ChannelFlavor::Zero(chan) => chan.sender().register(oper, cx),
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

    fn is_ready(&self) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().is_ready(),
            ChannelFlavor::List(chan) => chan.sender().is_ready(),
            ChannelFlavor::Zero(chan) => chan.sender().is_ready(),
        }
    }

    fn watch(&self, oper: Operation, cx: &Context) -> bool {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().watch(oper, cx),
            ChannelFlavor::List(chan) => chan.sender().watch(oper, cx),
            ChannelFlavor::Zero(chan) => chan.sender().watch(oper, cx),
        }
    }

    fn unwatch(&self, oper: Operation) {
        match &self.inner.flavor {
            ChannelFlavor::Array(chan) => chan.sender().unwatch(oper),
            ChannelFlavor::List(chan) => chan.sender().unwatch(oper),
            ChannelFlavor::Zero(chan) => chan.sender().unwatch(oper),
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
    fn try_select(&self, token: &mut Token) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().try_select(token),
                ChannelFlavor::List(chan) => chan.receiver().try_select(token),
                ChannelFlavor::Zero(chan) => chan.receiver().try_select(token),
            },
            ReceiverFlavor::After(chan) => chan.try_select(token),
            ReceiverFlavor::Tick(chan) => chan.try_select(token),
            ReceiverFlavor::Never(chan) => chan.try_select(token),
        }
    }

    fn deadline(&self) -> Option<Instant> {
        match &self.flavor {
            ReceiverFlavor::Channel(_) => None,
            ReceiverFlavor::After(chan) => chan.deadline(),
            ReceiverFlavor::Tick(chan) => chan.deadline(),
            ReceiverFlavor::Never(chan) => chan.deadline(),
        }
    }

    fn register(&self, oper: Operation, cx: &Context) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().register(oper, cx),
                ChannelFlavor::List(chan) => chan.receiver().register(oper, cx),
                ChannelFlavor::Zero(chan) => chan.receiver().register(oper, cx),
            },
            ReceiverFlavor::After(chan) => chan.register(oper, cx),
            ReceiverFlavor::Tick(chan) => chan.register(oper, cx),
            ReceiverFlavor::Never(chan) => chan.register(oper, cx),
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
            ReceiverFlavor::Never(chan) => chan.unregister(oper),
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
            ReceiverFlavor::Never(chan) => chan.accept(token, cx),
        }
    }

    fn is_ready(&self) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().is_ready(),
                ChannelFlavor::List(chan) => chan.receiver().is_ready(),
                ChannelFlavor::Zero(chan) => chan.receiver().is_ready(),
            },
            ReceiverFlavor::After(chan) => chan.is_ready(),
            ReceiverFlavor::Tick(chan) => chan.is_ready(),
            ReceiverFlavor::Never(chan) => chan.is_ready(),
        }
    }

    fn watch(&self, oper: Operation, cx: &Context) -> bool {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().watch(oper, cx),
                ChannelFlavor::List(chan) => chan.receiver().watch(oper, cx),
                ChannelFlavor::Zero(chan) => chan.receiver().watch(oper, cx),
            },
            ReceiverFlavor::After(chan) => chan.watch(oper, cx),
            ReceiverFlavor::Tick(chan) => chan.watch(oper, cx),
            ReceiverFlavor::Never(chan) => chan.watch(oper, cx),
        }
    }

    fn unwatch(&self, oper: Operation) {
        match &self.flavor {
            ReceiverFlavor::Channel(arc) => match &arc.flavor {
                ChannelFlavor::Array(chan) => chan.receiver().unwatch(oper),
                ChannelFlavor::List(chan) => chan.receiver().unwatch(oper),
                ChannelFlavor::Zero(chan) => chan.receiver().unwatch(oper),
            },
            ReceiverFlavor::After(chan) => chan.unwatch(oper),
            ReceiverFlavor::Tick(chan) => chan.unwatch(oper),
            ReceiverFlavor::Never(chan) => chan.unwatch(oper),
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
            ReceiverFlavor::Never(chan) => chan.state(),
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

/// Reads a message from the channel.
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
        ReceiverFlavor::Never(chan) => chan.read(token),
    }
}
