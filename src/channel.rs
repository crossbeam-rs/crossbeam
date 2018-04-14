use std::cmp;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;

use flavors;
use select::CaseId;
use utils::Backoff;

pub trait Sel {
    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool;
    fn promise(&self, case_id: CaseId);
    fn revoke(&self, case_id: CaseId);
    fn is_blocked(&self) -> bool;
    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool;
}
impl<'a, T: Sel> Sel for &'a T {
    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        (**self).try(token, backoff)
    }
    fn promise(&self, case_id: CaseId) {
        (**self).promise(case_id);
    }
    fn revoke(&self, case_id: CaseId) {
        (**self).revoke(case_id);
    }
    fn is_blocked(&self) -> bool {
        (**self).is_blocked()
    }
    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        (**self).fulfill(token, backoff)
    }
}

pub union Token {
    array: flavors::array::Token,
    list: flavors::list::Token,
    zero: flavors::zero::Token,
}

// pub type Token = usize;

// TODO: use backoff in try()/fulfill() for zero-capacity channels?

impl<T> Sel for Receiver<T> {
    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref chan) => chan.start_recv(&mut token.array, backoff),
                Flavor::List(ref chan) => chan.start_recv(&mut token.list, backoff),
                Flavor::Zero(ref chan) => chan.sel_try_recv(&mut token.zero),
            }
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

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref chan) => chan.start_recv(&mut token.array, backoff),
                Flavor::List(ref chan) => chan.start_recv(&mut token.list, backoff),
                Flavor::Zero(ref chan) => { token.zero = 1; true }, // TODO
            }
        }
    }
}

impl<T> Sel for Sender<T> {
    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref chan) => chan.start_send(&mut token.array, backoff),
                Flavor::List(_) => true,
                Flavor::Zero(ref chan) => chan.sel_try_send(&mut token.zero),
            }
        }
    }

    fn promise(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.senders().register(case_id),
            Flavor::List(_) => {},
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

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => unsafe { chan.start_send(&mut token.array, backoff) },
            Flavor::List(ref chan) => unsafe { token.list.entry = 0 as *const _; true }
            Flavor::Zero(ref chan) => unsafe { token.zero = 1; true },
        }
    }
}

pub struct Channel<T> {
    senders: AtomicUsize,
    flavor: Flavor<T>,
}

// TODO: rename to Channel?
enum Flavor<T> {
    Array(flavors::array::Channel<T>),
    List(flavors::list::Channel<T>),
    Zero(flavors::zero::Channel),
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
///     tx.send(fib(20));
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
/// tx.send(1);
///
/// thread::spawn(move || {
///     // This call blocks because the channel is full. It will be able to complete only after the
///     // first message is received.
///     tx.send(2);
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(rx.recv(), Some(1));
/// assert_eq!(rx.recv(), Some(2));
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
///     tx.send(1);
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(rx.recv(), Some(1));
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
///     tx1.send(1);
/// });
///
/// thread::spawn(move || {
///     tx2.send(2);
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

#[doc(hidden)]
impl<T> Sender<T> {
    fn new(chan: Arc<Channel<T>>) -> Self {
        chan.senders.fetch_add(1, SeqCst);
        Sender(chan)
    }

    fn channel_address(&self) -> usize {
        let chan: &Channel<T> = &*self.0;
        chan as *const Channel<T> as usize
    }

    pub unsafe fn finish_send(&self, token: Token, msg: T) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.finish_send(token.array, msg),
            Flavor::List(ref chan) => chan.send(msg, &mut Backoff::new()),
            Flavor::Zero(ref chan) => chan.finish_send(token.zero, msg),
        }
    }
}

impl<T> Sender<T> {
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
    /// use crossbeam_channel::bounded;
    ///
    /// let (tx, rx) = bounded(0);
    ///
    /// thread::spawn(move || {
    ///     tx.send(1);
    /// });
    ///
    /// assert_eq!(rx.recv(), Some(1));
    /// ```
    pub fn send(&self, msg: T) {
        select! {
            send(self, msg) => {}
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
    /// tx.send(0);
    /// assert!(!tx.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_empty(),
            Flavor::List(ref chan) => chan.is_empty(),
            Flavor::Zero(_) => true,
        }
    }

    /// Returns `true` if the channel is full.
    ///
    /// Zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::bounded;
    ///
    /// let (tx, rx) = bounded(1);
    ///
    /// assert!(!tx.is_full());
    /// tx.send(0);
    /// assert!(tx.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_full(),
            Flavor::List(ref chan) => false,
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
    /// tx.send(1);
    /// tx.send(2);
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
///     tx.send("Hello world!");
///     thread::sleep(Duration::from_secs(2)); // Block for two seconds.
///     tx.send("Delayed for 2 seconds");
/// });
///
/// println!("{}", rx.recv().unwrap()); // Received immediately.
/// println!("Waiting...");
/// println!("{}", rx.recv().unwrap()); // Received after 2 seconds.
/// ```
pub struct Receiver<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

#[doc(hidden)]
impl<T> Receiver<T> {
    fn new(chan: Arc<Channel<T>>) -> Self {
        Receiver(chan)
    }

    fn channel_address(&self) -> usize {
        let chan: &Channel<T> = &*self.0;
        chan as *const Channel<T> as usize
    }

    pub unsafe fn finish_recv(&self, token: Token) -> Option<T> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.finish_recv(token.array),
            Flavor::List(ref chan) => chan.finish_recv(token.list),
            Flavor::Zero(ref chan) => chan.finish_recv(token.zero),
        }
    }
}

impl<T> Receiver<T> {
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
    ///     tx.send(5);
    ///     drop(tx);
    /// });
    ///
    /// assert_eq!(rx.recv(), Some(5));
    /// assert_eq!(rx.recv(), None);
    /// ```
    pub fn recv(&self) -> Option<T> {
        select! {
            recv(self, msg) => msg
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
    /// use crossbeam_channel::unbounded;
    ///
    /// let (tx, rx) = unbounded();
    /// assert_eq!(rx.try_recv(), None);
    ///
    /// tx.send(5);
    /// drop(tx);
    ///
    /// assert_eq!(rx.try_recv(), Some(5));
    /// assert_eq!(rx.try_recv(), None);
    /// ```
    pub fn try_recv(&self) -> Option<T> {
        select! {
            recv(self, msg) => msg,
            default => None,
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
    ///
    /// assert!(rx.is_empty());
    /// tx.send(0);
    /// assert!(!rx.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_empty(),
            Flavor::List(ref chan) => chan.is_empty(),
            Flavor::Zero(_) => true,
        }
    }

    /// Returns `true` if the channel is full.
    ///
    /// Zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::bounded;
    ///
    /// let (tx, rx) = bounded(1);
    ///
    /// assert!(!rx.is_full());
    /// tx.send(0);
    /// assert!(rx.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_full(),
            Flavor::List(ref chan) => false,
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
    /// tx.send(1);
    /// tx.send(2);
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

impl<T> Iterator for Receiver<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv()
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Receiver<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.clone()
    }
}
