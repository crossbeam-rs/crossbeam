use std::cmp;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use flavors;
use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use select::CaseId;

use ::Sel;
impl<T> Sel for Receiver<T> {
    fn try(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.sel_try_recv(),
            Flavor::List(ref chan) => chan.sel_try_recv(),
            Flavor::Zero(ref chan) => chan.sel_try_recv(),
        }
    }

    fn promise(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.receivers().register(case_id),
            Flavor::List(ref chan) => chan.receivers().register(case_id),
            Flavor::Zero(ref chan) => chan.promise_recv(case_id),
        }
    }

    fn revoke(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.receivers().unregister(case_id),
            Flavor::List(ref chan) => chan.receivers().unregister(case_id),
            Flavor::Zero(ref chan) => chan.revoke_recv(case_id),
        }
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_empty() && !chan.is_closed(),
            Flavor::List(ref chan) => chan.is_empty() && !chan.is_closed(),
            Flavor::Zero(ref chan) => !chan.can_recv() && !chan.is_closed(),
        }
    }

    fn fulfill(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.sel_try_recv(),
            Flavor::List(ref chan) => chan.sel_try_recv(),
            Flavor::Zero(ref chan) => Some(1), // TODO
        }
    }
}

impl<T> Sel for Sender<T> {
    fn try(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.sel_try_send(),
            Flavor::List(ref chan) => Some(0),
            Flavor::Zero(ref chan) => chan.sel_try_send(),
        }
    }

    fn promise(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().register(case_id),
            Flavor::List(ref chan) => {},
            Flavor::Zero(ref chan) => chan.promise_send(case_id),
        }
    }

    fn revoke(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().unregister(case_id),
            Flavor::List(ref chan) => {},
            Flavor::Zero(ref chan) => chan.revoke_send(case_id),
        }
    }

    fn is_blocked(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_full(),
            Flavor::List(_) => true,
            Flavor::Zero(ref chan) => !chan.can_send(),
        }
    }

    fn fulfill(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.sel_try_send(),
            Flavor::List(ref chan) => Some(0),
            Flavor::Zero(ref chan) => Some(1), // TODO
        }
    }
}

#[test]
fn my() {
    let (s, r) = bounded::<i32>(0);
    ::std::thread::spawn(move || {
        println!("SENDING");
        s.send(7).unwrap();
        println!("SENT");
    });

    let sel: &Sel = &r;
    r.promise(r.case_id());
    println!("SLEEPING");
    ::std::thread::sleep_ms(1000);
    println!("WOKE UP");
    let token = sel.try();
    let v = unsafe { r.finish_recv(1) };

    assert_eq!(v, Some(7));
}

pub struct Channel<T> {
    senders: AtomicUsize,
    flavor: Flavor<T>,
}

enum Flavor<T> {
    Array(flavors::array::Channel<T>),
    List(flavors::list::Channel<T>),
    Zero(flavors::zero::Channel<T>),
}

/// Creates a new channel of unbounded capacity, returning the sender/receiver halves.
///
/// This type of channel can hold an unbounded number of messages, i.e. it has infinite capacity.
///
/// # Examples
///
/// ```
/// use crossbeam_channel::unbounded;
///
/// use std::thread;
///
/// let (tx, rx) = unbounded();
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
/// // Spawn a thread doing expensive computation.
/// thread::spawn(move || {
///     tx.send(fib(20)).unwrap();
/// });
///
/// // Do some useful work for a while...
///
/// // Let's see what's the result of the expensive computation.
/// println!("{}", rx.recv().unwrap());
/// ```
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        flavor: Flavor::List(flavors::list::Channel::new()),
    });
    (Sender::new(chan.clone()), Receiver::new(chan))
}

/// Creates a new channel of bounded capacity, returning the sender/receiver halves.
///
/// This type of channel has an internal buffer of length `cap` in messages get queued.
///
/// An interesting case is zero-capacity channel, also known as *rendezvous* channel. Such channel
/// cannot hold any messages, since its buffer is of length zero. Instead, send and receive
/// operations must execute at the same time in order to pair up and pass the message.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::bounded;
///
/// let (tx, rx) = bounded(1);
///
/// // This call returns immediately since there is enough space in the channel.
/// tx.send(1).unwrap();
///
/// thread::spawn(move || {
///     // This call blocks because the channel is full. It will be able to complete only after the
///     // first message is received.
///     tx.send(2).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(rx.recv(), Ok(1));
/// assert_eq!(rx.recv(), Ok(2));
/// ```
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::bounded;
///
/// let (tx, rx) = bounded(0);
///
/// thread::spawn(move || {
///     // This call blocks until the receive operation appears on the other end of the channel.
///     tx.send(1).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(rx.recv(), Ok(1));
/// ```
pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        flavor: {
            if cap == 0 {
                Flavor::Zero(flavors::zero::Channel::new())
            } else {
                Flavor::Array(flavors::array::Channel::with_capacity(cap))
            }
        },
    });
    (Sender::new(chan.clone()), Receiver::new(chan))
}

/// The sending half of a channel.
///
/// Senders can be cloned and shared among multiple threads.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (tx1, rx) = unbounded();
/// let tx2 = tx1.clone();
///
/// thread::spawn(move || {
///     tx1.send(1).unwrap();
/// });
///
/// thread::spawn(move || {
///     tx2.send(2).unwrap();
/// });
///
/// let msg1 = rx.recv().unwrap();
/// let msg2 = rx.recv().unwrap();
///
/// assert_eq!(3, msg1 + msg2);
/// ```
pub struct Sender<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

impl<T> Sender<T> {
    pub unsafe fn finish_send(&self, token: usize, msg: T) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.finish_send(token, msg),
            Flavor::List(ref chan) => chan.try_send(msg).unwrap(),
            Flavor::Zero(ref chan) => chan.finish_send(token, msg),
        }
    }

    fn new(chan: Arc<Channel<T>>) -> Self {
        chan.senders.fetch_add(1, SeqCst);
        Sender(chan)
    }

    fn channel_address(&self) -> usize {
        let chan: &Channel<T> = &*self.0;
        chan as *const Channel<T> as usize
    }

    #[doc(hidden)]
    pub fn case_id(&self) -> CaseId {
        CaseId::send(self.channel_address())
    }

    pub(crate) fn promise_send(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().register(self.case_id()),
            Flavor::List(_) => {}
            Flavor::Zero(ref chan) => chan.promise_send(self.case_id()),
        }
    }

    pub(crate) fn revoke_send(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().unregister(self.case_id()),
            Flavor::List(_) => {}
            Flavor::Zero(ref chan) => chan.revoke_send(self.case_id()),
        }
    }

    pub(crate) fn can_send(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => !chan.is_full(),
            Flavor::List(_) => true,
            Flavor::Zero(ref chan) => chan.can_send(),
        }
    }

    pub(crate) fn fulfill_send(&self, msg: T) -> Result<(), T> {
        match self.0.flavor {
            Flavor::Array(_) | Flavor::List(_) => match self.try_send(msg) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(m)) => Err(m),
                Err(TrySendError::Closed(m)) => Err(m),
            },
            Flavor::Zero(ref chan) => {
                chan.fulfill_send(msg);
                Ok(())
            }
        }
    }

    /// Attempts to send a message into the channel without blocking.
    ///
    /// This method will either send a message into the channel immediately, or return an error if
    /// the channel is full or closed. The returned error contains the original message.
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
    /// ```
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.try_send(msg),
            Flavor::List(ref chan) => chan.try_send(msg),
            Flavor::Zero(ref chan) => chan.try_send(msg, self.case_id()),
        }
    }

    /// Sends a message into the channel, blocking if the channel is full.
    ///
    /// If the channel is full (its capacity is fully utilized), this call will block until the
    /// send operation can proceed. If the channel is (or gets) closed, this call will wake up and
    /// return an error.
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
    /// let (tx, rx) = bounded(1);
    /// assert_eq!(tx.send(1), Ok(()));
    ///
    /// thread::spawn(move || {
    ///     assert_eq!(rx.recv(), Ok(1));
    ///     thread::sleep(Duration::from_secs(1));
    ///     //drop(rx);
    /// });
    ///
    /// assert_eq!(tx.send(2), Ok(()));
    /// //assert_eq!(tx.send(3), Err(SendError(3)));
    /// ```
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        let res = match self.0.flavor {
            Flavor::Array(ref chan) => chan.send_until(msg, None, self.case_id()),
            Flavor::List(ref chan) => chan.send(msg),
            Flavor::Zero(ref chan) => chan.send_until(msg, None, self.case_id()),
        };
        match res {
            Ok(()) => Ok(()),
            Err(SendTimeoutError::Closed(m)) => Err(SendError(m)),
            Err(SendTimeoutError::Timeout(m)) => Err(SendError(m)),
        }
    }

    /// Sends a message into the channel, blocking if the channel is full for a limited time.
    ///
    /// If the channel is full (its capacity is fully utilized), this call will block until the
    /// send operation can proceed. If the channel is (or gets) closed, or if it waits for longer
    /// than `timeout`, this call will wake up and return an error.
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
    /// assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Closed));
    /// ```
    pub fn send_timeout(&self, msg: T, timeout: Duration) -> Result<(), SendTimeoutError<T>> {
        let deadline = Some(Instant::now() + timeout);
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.send_until(msg, deadline, self.case_id()),
            Flavor::List(ref chan) => chan.send(msg),
            Flavor::Zero(ref chan) => chan.send_until(msg, deadline, self.case_id()),
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded();
    /// assert!(tx.is_empty());
    ///
    /// tx.send(0).unwrap();
    /// assert!(!tx.is_empty());
    ///
    /// // Drop the only receiver, thus closing the channel.
    /// drop(rx);
    /// // Even a closed channel can be non-empty.
    /// assert!(!tx.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_empty(),
            Flavor::List(ref chan) => chan.is_empty(),
            Flavor::Zero(_) => true,
        }
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded();
    /// assert_eq!(tx.len(), 0);
    ///
    /// tx.send(1).unwrap();
    /// tx.send(2).unwrap();
    /// assert_eq!(tx.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.len(),
            Flavor::List(ref chan) => chan.len(),
            Flavor::Zero(_) => 0,
        }
    }

    /// If the channel is bounded, returns its capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, unbounded};
    ///
    /// let (tx, _) = unbounded::<i32>();
    /// assert_eq!(tx.capacity(), None);
    ///
    /// let (tx, _) = bounded::<i32>(5);
    /// assert_eq!(tx.capacity(), Some(5));
    ///
    /// let (tx, _) = bounded::<i32>(0);
    /// assert_eq!(tx.capacity(), Some(0));
    /// ```
    pub fn capacity(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => Some(chan.capacity()),
            Flavor::List(_) => None,
            Flavor::Zero(_) => Some(0),
        }
    }

    #[doc(hidden)]
    pub fn is_closed(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_closed(),
            Flavor::List(ref chan) => chan.is_closed(),
            Flavor::Zero(ref chan) => chan.is_closed(),
        }
    }

    /// Closes the channel.
    ///
    /// Returns `true` if this call closed the channel and `false` if it was already closed.
    ///
    /// Closing prevents any further messages from being sent into the channel, while still
    /// allowing the receiver to drain any existing buffered messages.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded::<i32>();
    /// tx.send(1);
    /// tx.send(2);
    ///
    /// rx.close();
    /// assert_eq!(rx.recv(), Ok(1));
    /// assert_eq!(rx.recv(), Ok(2));
    /// assert!(rx.recv().is_err());
    /// ```
    pub fn close(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.close(),
            Flavor::List(ref chan) => chan.close(),
            Flavor::Zero(ref chan) => chan.close(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, SeqCst) == 1 {
            match self.0.flavor {
                Flavor::Array(ref chan) => chan.close(),
                Flavor::List(ref chan) => chan.close(),
                Flavor::Zero(ref chan) => chan.close(),
            };
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender::new(self.0.clone())
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Sender").finish()
    }
}

impl<T> Hash for Sender<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.channel_address().hash(state)
    }
}

impl<T> PartialEq for Sender<T> {
    fn eq(&self, other: &Sender<T>) -> bool {
        self.channel_address() == other.channel_address()
    }
}

impl<T> Eq for Sender<T> {}

impl<T> PartialOrd for Sender<T> {
    fn partial_cmp(&self, other: &Sender<T>) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Sender<T> {
    fn cmp(&self, other: &Sender<T>) -> cmp::Ordering {
        self.channel_address().cmp(&other.channel_address())
    }
}

/// The receiving half of a channel.
///
/// Receivers can be cloned and shared among multiple threads.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::unbounded;
///
/// let (tx, rx) = unbounded();
///
/// thread::spawn(move || {
///     tx.send("Hello world!").unwrap();
///     thread::sleep(Duration::from_secs(2)); // Block for two seconds.
///     tx.send("Delayed for 2 seconds").unwrap();
/// });
///
/// println!("{}", rx.recv().unwrap()); // Received immediately.
/// println!("Waiting...");
/// println!("{}", rx.recv().unwrap()); // Received after 2 seconds.
/// ```
pub struct Receiver<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

impl<T> Receiver<T> {
    pub unsafe fn finish_recv(&self, token: usize) -> Option<T> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.finish_recv(token),
            Flavor::List(ref chan) => chan.finish_recv(token),
            Flavor::Zero(ref chan) => chan.finish_recv(token),
        }
    }

    fn new(chan: Arc<Channel<T>>) -> Self {
        Receiver(chan)
    }

    fn channel_address(&self) -> usize {
        let chan: &Channel<T> = &*self.0;
        chan as *const Channel<T> as usize
    }

    #[doc(hidden)]
    pub fn case_id(&self) -> CaseId {
        CaseId::recv(self.channel_address())
    }

    pub(crate) fn promise_recv(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.receivers().register(self.case_id()),
            Flavor::List(ref chan) => chan.receivers().register(self.case_id()),
            Flavor::Zero(ref chan) => chan.promise_recv(self.case_id()),
        }
    }

    pub(crate) fn revoke_recv(&self) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.receivers().unregister(self.case_id()),
            Flavor::List(ref chan) => chan.receivers().unregister(self.case_id()),
            Flavor::Zero(ref chan) => chan.revoke_recv(self.case_id()),
        }
    }

    pub(crate) fn can_recv(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => !chan.is_empty(),
            Flavor::List(ref chan) => !chan.is_empty(),
            Flavor::Zero(ref chan) => chan.can_recv(),
        }
    }

    pub(crate) fn fulfill_recv(&self) -> Result<T, ()> {
        match self.0.flavor {
            Flavor::Array(_) | Flavor::List(_) => self.try_recv().map_err(|_| ()),
            Flavor::Zero(ref chan) => Ok(chan.fulfill_recv()),
        }
    }

    /// Attempts to receive a message from the channel without blocking.
    ///
    /// This method will never block in order to wait for a message to become available. Instead,
    /// this will always return immediately with a message if there is one, or an error if the
    /// channel is empty or closed.
    ///
    /// If called on a zero-capacity channel, this method will receive a message only if there
    /// happens to be a send operation on the other side of the channel at the same time.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, TryRecvError};
    ///
    /// let (tx, rx) = unbounded();
    /// assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    ///
    /// tx.send(5).unwrap();
    /// drop(tx);
    ///
    /// assert_eq!(rx.try_recv(), Ok(5));
    /// assert_eq!(rx.try_recv(), Err(TryRecvError::Closed));
    /// ```
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.try_recv(),
            Flavor::List(ref chan) => chan.try_recv(),
            Flavor::Zero(ref chan) => chan.try_recv(self.case_id()),
        }
    }

    /// Waits for a message to be received from the channel.
    ///
    /// This method will always block in order to wait for a message to become available. If the
    /// channel is (or gets) closed and empty, this call will wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a send operation to appear
    /// on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     tx.send(5).unwrap();
    ///     drop(tx);
    /// });
    ///
    /// assert_eq!(rx.recv(), Ok(5));
    /// assert!(rx.recv().is_err());
    /// ```
    pub fn recv(&self) -> Result<T, RecvError> {
        let res = match self.0.flavor {
            Flavor::Array(ref chan) => chan.recv_until(None, self.case_id()),
            Flavor::List(ref chan) => chan.recv_until(None, self.case_id()),
            Flavor::Zero(ref chan) => chan.recv_until(None, self.case_id()),
        };
        if let Ok(m) = res {
            Ok(m)
        } else {
            Err(RecvError)
        }
    }

    /// Waits for a message to be received from the channel but only for a limited time.
    ///
    /// This method will always block in order to wait for a message to become available. If the
    /// channel is (or gets) closed and empty, or if it waits for longer than `timeout`, it will
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
    /// assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Err(RecvTimeoutError::Closed));
    /// ```
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        let deadline = Some(Instant::now() + timeout);
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.recv_until(deadline, self.case_id()),
            Flavor::List(ref chan) => chan.recv_until(deadline, self.case_id()),
            Flavor::Zero(ref chan) => chan.recv_until(deadline, self.case_id()),
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded();
    /// assert!(rx.is_empty());
    ///
    /// tx.send(0).unwrap();
    /// assert!(!rx.is_empty());
    ///
    /// // Drop the only sender, thus closing the channel.
    /// drop(tx);
    /// // Even a closed channel can be non-empty.
    /// assert!(!rx.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_empty(),
            Flavor::List(ref chan) => chan.is_empty(),
            Flavor::Zero(_) => true,
        }
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded();
    /// assert_eq!(rx.len(), 0);
    ///
    /// tx.send(1).unwrap();
    /// tx.send(2).unwrap();
    /// assert_eq!(rx.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.len(),
            Flavor::List(ref chan) => chan.len(),
            Flavor::Zero(_) => 0,
        }
    }

    /// If the channel is bounded, returns its capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, unbounded};
    ///
    /// let (tx, _) = unbounded::<i32>();
    /// assert_eq!(tx.capacity(), None);
    ///
    /// let (tx, _) = bounded::<i32>(5);
    /// assert_eq!(tx.capacity(), Some(5));
    ///
    /// let (tx, _) = bounded::<i32>(0);
    /// assert_eq!(tx.capacity(), Some(0));
    /// ```
    pub fn capacity(&self) -> Option<usize> {
        match self.0.flavor {
            Flavor::Array(ref chan) => Some(chan.capacity()),
            Flavor::List(_) => None,
            Flavor::Zero(_) => Some(0),
        }
    }

    #[doc(hidden)]
    pub fn is_closed(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_closed(),
            Flavor::List(ref chan) => chan.is_closed(),
            Flavor::Zero(ref chan) => chan.is_closed(),
        }
    }

    /// Returns an iterator that waits for messages until the channel is closed.
    ///
    /// Each call to `next` will block waiting for the next message. It will finally return `None`
    /// when the channel is empty and closed.
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

    /// Returns an iterator that receives messages until the channel is empty or closed.
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

    /// Closes the channel.
    ///
    /// Returns `true` if this call closed the channel and `false` if it was already closed.
    ///
    /// Closing prevents any further messages from being sent into the channel, while still
    /// allowing the receiver to drain any existing buffered messages.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded::<i32>();
    /// tx.send(1);
    /// tx.send(2);
    /// tx.close();
    ///
    /// assert_eq!(rx.recv(), Ok(1));
    /// assert_eq!(rx.recv(), Ok(2));
    /// assert!(rx.recv().is_err());
    /// ```
    pub fn close(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.close(),
            Flavor::List(ref chan) => chan.close(),
            Flavor::Zero(ref chan) => chan.close(),
        }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Receiver::new(self.0.clone())
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Receiver").finish()
    }
}

impl<T> Hash for Receiver<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.channel_address().hash(state)
    }
}

impl<T> PartialEq for Receiver<T> {
    fn eq(&self, other: &Receiver<T>) -> bool {
        self.channel_address() == other.channel_address()
    }
}

impl<T> Eq for Receiver<T> {}

impl<T> PartialOrd for Receiver<T> {
    fn partial_cmp(&self, other: &Receiver<T>) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Receiver<T> {
    fn cmp(&self, other: &Receiver<T>) -> cmp::Ordering {
        self.channel_address().cmp(&other.channel_address())
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

/// An iterator that waits for messages until the channel is closed.
///
/// Each call to `next` will block waiting for the next message. It will finally return `None` when
/// the channel is empty and closed.
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

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}

/// An iterator that receives messages until the channel is empty or closed.
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

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.try_recv().ok()
    }
}

/// An owning iterator that waits for messages until the channel is closed.
///
/// Each call to `next` will block waiting for the next message. It will finally return `None` when
/// the channel is empty and closed.
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

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.rx.recv().ok()
    }
}
