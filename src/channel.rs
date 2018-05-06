use std::cmp;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use flavors;
use select::CaseId;
use select::Sel;
use utils::Backoff;

// TODO: explain
// loop { try; promise; is_blocked; revoke; fulfill }
// -> write; fail or finish
// -> read; finish

pub union Token {
    array: flavors::array::Token,
    list: flavors::list::Token,
    zero: flavors::zero::Token,
}

// TODO: use backoff in try()/fulfill() for zero-capacity channels?

impl<T> Sel for Receiver<T> {
    type Token = Token;

    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.receiver().try(&mut token.array, backoff),
                Flavor::List(ref inner) => inner.receiver().try(&mut token.list, backoff),
                Flavor::Zero(ref inner) => inner.receiver().try(&mut token.zero, backoff),
            }
        }
    }

    fn promise(&self, token: &mut Token ,case_id: CaseId) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.receiver().promise(&mut token.array, case_id),
                Flavor::List(ref inner) => inner.receiver().promise(&mut token.list, case_id),
                Flavor::Zero(ref inner) => inner.receiver().promise(&mut token.zero, case_id),
            }
        }
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        match self.0.flavor {
            Flavor::Array(ref inner) => inner.receiver().is_blocked(),
            Flavor::List(ref inner) => inner.receiver().is_blocked(),
            Flavor::Zero(ref inner) => inner.receiver().is_blocked(),
        }
    }

    fn revoke(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref inner) => inner.receiver().revoke(case_id),
            Flavor::List(ref inner) => inner.receiver().revoke(case_id),
            Flavor::Zero(ref inner) => inner.receiver().revoke(case_id),
        }
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.receiver().fulfill(&mut token.array, backoff),
                Flavor::List(ref inner) => inner.receiver().fulfill(&mut token.list, backoff),
                Flavor::Zero(ref inner) => inner.receiver().fulfill(&mut token.zero, backoff),
            }
        }
    }

    fn finish(&self, token: &mut Token) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.receiver().finish(&mut token.array),
                Flavor::List(ref inner) => inner.receiver().finish(&mut token.list),
                Flavor::Zero(ref inner) => inner.receiver().finish(&mut token.zero),
            }
        }
    }

    fn fail(&self, _token: &mut Token) {
        unreachable!();
    }
}

impl<T> Sel for Sender<T> {
    type Token = Token;

    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.sender().try(&mut token.array, backoff),
                Flavor::List(ref inner) => inner.sender().try(&mut token.list, backoff),
                Flavor::Zero(ref inner) => inner.sender().try(&mut token.zero, backoff),
            }
        }
    }

    fn promise(&self, token: &mut Token, case_id: CaseId) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.sender().promise(&mut token.array, case_id),
                Flavor::List(ref inner) => inner.sender().promise(&mut token.list, case_id),
                Flavor::Zero(ref inner) => inner.sender().promise(&mut token.zero, case_id),
            }
        }
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        match self.0.flavor {
            Flavor::Array(ref inner) => inner.sender().is_blocked(),
            Flavor::List(ref inner) => inner.sender().is_blocked(),
            Flavor::Zero(ref inner) => inner.sender().is_blocked(),
        }
    }

    fn revoke(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref inner) => inner.sender().revoke(case_id),
            Flavor::List(ref inner) => inner.sender().revoke(case_id),
            Flavor::Zero(ref inner) => inner.sender().revoke(case_id),
        }
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.sender().fulfill(&mut token.array, backoff),
                Flavor::List(ref inner) => inner.sender().fulfill(&mut token.list, backoff),
                Flavor::Zero(ref inner) => inner.sender().fulfill(&mut token.zero, backoff),
            }
        }
    }

    fn finish(&self, token: &mut Token) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.sender().finish(&mut token.array),
                Flavor::List(ref inner) => inner.sender().finish(&mut token.list),
                Flavor::Zero(ref inner) => inner.sender().finish(&mut token.zero),
            }
        }
    }

    fn fail(&self, token: &mut Token) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.sender().fail(&mut token.array),
                Flavor::List(ref inner) => inner.sender().fail(&mut token.list),
                Flavor::Zero(ref inner) => inner.sender().fail(&mut token.zero),
            }
        }
    }
}

pub struct PreparedSender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> Sel for PreparedSender<'a, T> {
    type Token = Token;

    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.prepared_sender().try(&mut token.array, backoff),
                Flavor::List(ref inner) => inner.prepared_sender().try(&mut token.list, backoff),
                Flavor::Zero(ref inner) => inner.prepared_sender().try(&mut token.zero, backoff),
            }
        }
    }

    fn promise(&self, token: &mut Token, case_id: CaseId) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.prepared_sender().promise(&mut token.array, case_id),
                Flavor::List(ref inner) => inner.prepared_sender().promise(&mut token.list, case_id),
                Flavor::Zero(ref inner) => inner.prepared_sender().promise(&mut token.zero, case_id),
            }
        }
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        match self.0.flavor {
            Flavor::Array(ref inner) => inner.prepared_sender().is_blocked(),
            Flavor::List(ref inner) => inner.prepared_sender().is_blocked(),
            Flavor::Zero(ref inner) => inner.prepared_sender().is_blocked(),
        }
    }

    fn revoke(&self, case_id: CaseId) {
        match self.0.flavor {
            Flavor::Array(ref inner) => inner.prepared_sender().revoke(case_id),
            Flavor::List(ref inner) => inner.prepared_sender().revoke(case_id),
            Flavor::Zero(ref inner) => inner.prepared_sender().revoke(case_id),
        }
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.prepared_sender().fulfill(&mut token.array, backoff),
                Flavor::List(ref inner) => inner.prepared_sender().fulfill(&mut token.list, backoff),
                Flavor::Zero(ref inner) => inner.prepared_sender().fulfill(&mut token.zero, backoff),
            }
        }
    }

    fn finish(&self, token: &mut Token) {
        unsafe {
            match self.0.flavor {
                Flavor::Array(ref inner) => inner.prepared_sender().finish(&mut token.array),
                Flavor::List(ref inner) => inner.prepared_sender().finish(&mut token.list),
                Flavor::Zero(ref inner) => inner.prepared_sender().finish(&mut token.zero),
            }
        }
    }

    fn fail(&self, _token: &mut Token) {
        process::abort();
    }
}

#[doc(hidden)]
impl<'a, T> PreparedSender<'a, T> {
    fn channel_id(&self) -> usize {
        &*self.0 as *const Channel<T> as usize
    }

    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.write(&mut token.array, msg, true),
            Flavor::List(ref chan) => chan.write(&mut token.list, msg),
            Flavor::Zero(ref chan) => chan.write(&mut token.zero, msg, true),
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
    Zero(flavors::zero::Channel<T>),
}

/// Creates a new channel of unbounded capacity, returning the sender and receiver halves.
///
/// This type of channel can hold any number of messages; i.e. it has infinite capacity.
///
/// # Examples
///
/// ```
/// use crossbeam_channel::unbounded;
///
/// use std::thread;
///
/// let (s, r) = unbounded();
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
///     s.send(fib(20));
/// });
///
/// // Let's see what's the result of the computation.
/// println!("{}", r.recv().unwrap());
/// ```
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Channel {
        senders: AtomicUsize::new(0),
        flavor: Flavor::List(flavors::list::Channel::new()),
    });
    (Sender::new(chan.clone()), Receiver::new(chan))
}

/// Creates a new channel of bounded capacity, returning the sender and receiver halves.
///
/// This type of channel has an internal buffer of length `cap` in which messages get queued.
///
/// An interesting case is zero-capacity channel, also known as *rendezvous* channel. Such a
/// channel cannot hold any messages since its buffer is of length zero. Instead, send and receive
/// operations must be executing at the same time in order to pair up and pass the message.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::bounded;
///
/// let (s, r) = bounded(1);
///
/// // This call returns immediately since there is enough space in the channel.
/// s.send(1);
///
/// thread::spawn(move || {
///     // This call blocks because the channel is full. It will be able to complete only after the
///     // first message is received.
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
/// use crossbeam_channel::bounded;
///
/// let (s, r) = bounded(0);
///
/// thread::spawn(move || {
///     // This call blocks until the receive operation appears on the other end of the channel.
///     s.send(1);
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Some(1));
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
/// let (s1, r) = unbounded();
/// let s2 = s1.clone();
///
/// thread::spawn(move || {
///     s1.send(1);
/// });
///
/// thread::spawn(move || {
///     s2.send(2);
/// });
///
/// let msg1 = r.recv().unwrap();
/// let msg2 = r.recv().unwrap();
///
/// assert_eq!(3, msg1 + msg2);
/// ```
pub struct Sender<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Sync for Sender<T> {}

#[doc(hidden)]
impl<T> Sender<T> {
    fn new(chan: Arc<Channel<T>>) -> Self {
        const MAX_REFCOUNT: usize = (::std::isize::MAX) as usize;

        let old_count = chan.senders.fetch_add(1, Ordering::SeqCst);

        // See comments on Arc::clone() on why we do this (for `mem::forget`).
        if old_count > MAX_REFCOUNT {
            process::abort();
        }

        Sender(chan)
    }

    fn channel_id(&self) -> usize {
        &*self.0 as *const Channel<T> as usize
    }

    // TODO: fn write_send() and fn read_recv(), then finish() with just token
    // TODO: impl these methods for SendLiteral, too!
    // TODO: move finish and fail into Sel?

    pub unsafe fn write(&self, token: &mut Token, msg: T) {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.write(&mut token.array, msg, false),
            Flavor::List(ref chan) => chan.write(&mut token.list, msg),
            Flavor::Zero(ref chan) => chan.write(&mut token.zero, msg, false),
        }
    }
}

impl<T> Sender<T> {
    /// Sends a message into the channel, blocking if the channel is full.
    ///
    /// If called on a zero-capacity channel, this method blocks until a receive operation appears
    /// on the other side of the channel.
    ///
    /// Note that `s.send(msg)` is equivalent to the following:
    ///
    /// ```ignore
    /// select! {
    ///     send(s, msg) => {}
    /// }
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::bounded;
    ///
    /// let (s, r) = bounded(0);
    ///
    /// thread::spawn(move || {
    ///     s.send(1);
    /// });
    ///
    /// assert_eq!(r.recv(), Some(1));
    /// ```
    pub fn send(&self, msg: T) {
        select! {
            send(PreparedSender(&self.0), msg) => {}
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    /// assert!(s.is_empty());
    ///
    /// s.send(0);
    /// assert!(!s.is_empty());
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
    /// Note: zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::bounded;
    ///
    /// let (s, r) = bounded(1);
    ///
    /// assert!(!s.is_full());
    /// s.send(0);
    /// assert!(s.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_full(),
            Flavor::List(_) => false,
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
    /// let (s, r) = unbounded();
    /// assert_eq!(s.len(), 0);
    ///
    /// s.send(1);
    /// s.send(2);
    /// assert_eq!(s.len(), 2);
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
        match self.0.flavor {
            Flavor::Array(ref chan) => Some(chan.capacity()),
            Flavor::List(_) => None,
            Flavor::Zero(_) => Some(0),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.0.senders.fetch_sub(1, Ordering::SeqCst) == 1 {
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
        self.channel_id().hash(state)
    }
}

impl<T> PartialEq for Sender<T> {
    fn eq(&self, other: &Sender<T>) -> bool {
        self.channel_id() == other.channel_id()
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
        self.channel_id().cmp(&other.channel_id())
    }
}

impl<T> PartialEq<Receiver<T>> for Sender<T> {
    fn eq(&self, other: &Receiver<T>) -> bool {
        self.channel_id() == other.channel_id()
    }
}

impl<T> PartialOrd<Receiver<T>> for Sender<T> {
    fn partial_cmp(&self, other: &Receiver<T>) -> Option<cmp::Ordering> {
        self.channel_id().partial_cmp(&other.channel_id())
    }
}

impl<T> UnwindSafe for Sender<T> {}

impl<T> RefUnwindSafe for Sender<T> {}

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
/// let (s, r) = unbounded();
///
/// thread::spawn(move || {
///     s.send("Hello world!");
///     thread::sleep(Duration::from_secs(2)); // Block for two seconds.
///     s.send("Delayed for 2 seconds");
/// });
///
/// println!("{}", r.recv().unwrap()); // Received immediately.
/// println!("Waiting...");
/// println!("{}", r.recv().unwrap()); // Received after 2 seconds.
/// ```
pub struct Receiver<T>(Arc<Channel<T>>);

unsafe impl<T: Send> Send for Receiver<T> {}
unsafe impl<T: Send> Sync for Receiver<T> {}

#[doc(hidden)]
impl<T> Receiver<T> {
    fn new(chan: Arc<Channel<T>>) -> Self {
        Receiver(chan)
    }

    fn channel_id(&self) -> usize {
        &*self.0 as *const Channel<T> as usize
    }

    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.read(&mut token.array),
            Flavor::List(ref chan) => chan.read(&mut token.list),
            Flavor::Zero(ref chan) => chan.read(&mut token.zero),
        }
    }
}

impl<T> Receiver<T> {
    /// Blocks until a message is received or the channel is closed.
    ///
    /// Returns the message if it was received or `None` if the channel is closed.
    ///
    /// If called on a zero-capacity channel, this method blocks until a send operation appears on
    /// the other side of the channel.
    ///
    /// Note that `r.recv()` is equivalent to the following:
    ///
    /// ```ignore
    /// select! {
    ///     recv(r, msg) => msg
    /// }
    /// ```
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
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5);
    ///     // `s` gets dropped, closing the channel.
    /// });
    ///
    /// assert_eq!(r.recv(), Some(5));
    /// assert_eq!(r.recv(), None);
    /// ```
    pub fn recv(&self) -> Option<T> {
        // TODO: select_internal!(@mainloop recv() ())
        // match self.0.flavor {
        //     Flavor::Array(ref chan) => select! { recv(chan.receiver(), msg) => msg },
        //     Flavor::List(ref chan) => select! { recv(chan.receiver(), msg) => msg },
        //     Flavor::Zero(_) => select! { recv(chan.receiver(), msg) => msg },
        // }
        select! {
            recv(self, msg) => msg
        }
    }

    /// Attempts to receive a message from the channel without blocking.
    ///
    /// If there is no message ready to be received or the channel is closed, returns `None`.
    ///
    /// Note that `r.try_recv()` is just a shorter version of the following:
    ///
    /// ```ignore
    /// select! {
    ///     recv(r, msg) => msg,
    ///     default => None,
    /// }
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    /// assert_eq!(r.try_recv(), None);
    ///
    /// s.send(5);
    /// drop(s);
    ///
    /// assert_eq!(r.try_recv(), Some(5));
    /// assert_eq!(r.try_recv(), None);
    /// ```
    pub fn try_recv(&self) -> Option<T> {
        select! {
            recv(self, msg) => msg,
            default => None,
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::unbounded;
    ///
    /// let (s, r) = unbounded();
    ///
    /// assert!(r.is_empty());
    /// s.send(0);
    /// assert!(!r.is_empty());
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
    /// Note: zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::bounded;
    ///
    /// let (s, r) = bounded(1);
    ///
    /// assert!(!r.is_full());
    /// s.send(0);
    /// assert!(r.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        match self.0.flavor {
            Flavor::Array(ref chan) => chan.is_full(),
            Flavor::List(_) => false,
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
    /// let (s, r) = unbounded();
    /// assert_eq!(r.len(), 0);
    ///
    /// s.send(1);
    /// s.send(2);
    /// assert_eq!(r.len(), 2);
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
        self.channel_id().hash(state)
    }
}

impl<T> PartialEq for Receiver<T> {
    fn eq(&self, other: &Receiver<T>) -> bool {
        self.channel_id() == other.channel_id()
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
        self.channel_id().cmp(&other.channel_id())
    }
}

impl<T> PartialEq<Sender<T>> for Receiver<T> {
    fn eq(&self, other: &Sender<T>) -> bool {
        self.channel_id() == other.channel_id()
    }
}

impl<T> PartialOrd<Sender<T>> for Receiver<T> {
    fn partial_cmp(&self, other: &Sender<T>) -> Option<cmp::Ordering> {
        self.channel_id().partial_cmp(&other.channel_id())
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

impl<T> UnwindSafe for Receiver<T> {}

impl<T> RefUnwindSafe for Receiver<T> {}
