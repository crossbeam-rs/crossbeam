use std::time::Instant;

use flavors;

/// Waits on a set of channel operations.
///
/// This macro allows one to declare a set of channel operations and block until any one of them
/// becomes ready. Finally, one of the operations is executed. If multiple operations are ready at
/// the same time, a random one is chosen. It is also possible to declare a `default` case that
/// gets executed if none of the operations are initially ready.
///
/// # Receiving
///
/// Receiving a message from two channels, whichever becomes ready first:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// thread::spawn(move || s1.send("foo"));
/// thread::spawn(move || s2.send("bar"));
///
/// // Only one of these two receive operations will be executed.
/// select! {
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
///     recv(r2, msg) => assert_eq!(msg, Some("bar")),
/// }
/// # }
/// ```
///
/// # Sending
///
/// Waiting on a send and a receive operation:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// s1.send("foo");
///
/// // Since both operations are ready, a random one will be executed.
/// select! {
///     send(s2, "bar") => assert_eq!(r2.recv(), Some("bar")),
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
/// }
/// # }
/// ```
///
/// # Default case
///
/// A special kind of case is `default`, which gets executed if none of the operations can be
/// executed, i.e. they would block:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::unbounded();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     s.send("foo");
/// });
///
/// // Don't block on the receive operation.
/// select! {
///     recv(r) => panic!(),
///     default => println!("The message is not yet available."),
/// }
/// # }
/// ```
///
/// # Iterators
///
/// It is possible to have arbitrary iterators of senders or receivers in a single `send` or `recv`
/// case:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// s1.send("foo");
/// s2.send("bar");
/// let receivers = vec![r1, r2];
///
/// // Both receivers are ready so one of the two receive operations will be chosen at random.
/// select! {
///     // The third argument to `recv` is optional and is assigned a reference to the receiver
///     // the message was received from.
///     recv(receivers, msg, fired) => {
///         for (i, r) in receivers.iter().enumerate() {
///             if r == fired {
///                 println!("Received {:?} from the {}-th receiver.", msg, i);
///             }
///         }
///     }
/// }
/// # }
/// ```
///
/// # Performance
///
/// Methods [`try_recv`], [`recv`], and [`send`] are typically faster than `select!` because
/// they're not as general and have better optimized implementations.
///
/// If performance is of high concern, you might try calling one of those methods before falling
/// back to `select!`, like in the following example:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::time::Duration;
/// use crossbeam_channel as channel;
///
/// fn recv_timeout<T>(r: &channel::Receiver<T>, timeout: Duration) -> Option<T> {
///     // Optimistic fast path.
///     if let Some(msg) = r.try_recv() {
///         return Some(msg);
///     }
///
///     // Falling back to slower `select!`.
///     select! {
///         recv(r, msg) => msg,
///         recv(channel::after(timeout)) => None,
///     }
/// }
/// # }
/// ```
///
/// # Syntax
///
/// TODO: maybe model after https://golang.org/ref/spec#Select_statements
///
/// An invocation of `select!` consists of a list of cases. Consecutive cases are delimited by a
/// comma, but it's not required if the previous case has a block expression (the syntax is very
/// similar to `match`).
///
/// There are three types of cases: `recv`, `send`, and `default`.
///
/// ### `recv` case
///
/// The syntax of a `recv` case takes one of these forms:
///
/// 1. `recv(r) => body`
/// 2. `recv(r, msg) => body`
/// 3. `recv(r, msg, fired) => body`
///
/// Inputs: expressions `r` and `body`.
///
/// Outputs: patterns `msg` and `fired`.
///
/// Types:
///
/// * `r`: one of `Receiver<T>`, `&Receiver<T>`, or `impl IntoIterator<Item = &Receiver<T>>`
/// * `msg`: `Option<T>`
/// * `fired`: `&Receiver<T>`
///
/// ### `send` case
///
/// The syntax of a `send` case takes one of these forms:
///
/// 1. `send(s, msg) => body`
/// 2. `send(s, msg, fired) => body`
///
/// Inputs: expressions `s`, `msg`, and `body`.
///
/// Outputs: pattern `fired`.
///
/// Types:
///
/// * `s`: one of `Sender<T>`, `&Sender<T>`, or `impl IntoIterator<Item = &Sender<T>>`
/// * `msg`: `T`
/// * `fired`: `&Sender<T>`
///
/// ### `default` case
///
/// The default case takes one of these two forms:
///
/// 1. `default => body`
/// 2. `default() => body`
///
/// Inputs: expression `body`.
///
/// There can be at most one default case.
///
/// # Behaviora
///
/// TODO: model after https://golang.org/ref/spec#Select_statements
///
/// First, all sender and receiver arguments (`s` and `r`) are evaluated. Then, the current thread
/// is blocked until one of the operations becomes ready, which is then executed.
///
/// If a `recv` operation gets executed, `msg` and `fired` are assigned, and `body` is finally
/// evaluated.
///
/// If a `send` operation gets executed, `msg` is evaluated and sent into the channel, `fired` is
/// assigned, and `body` is finally evaluated.
///
/// **Note**: If evaluation of `msg` panics, the program will be aborted because it's very
/// difficult to sensibly recover from the panic.
///
/// [`send`]: struct.Sender.html#method.send
/// [`try_recv`]: struct.Receiver.html#method.try_recv
/// [`recv`]: struct.Receiver.html#method.recv
#[macro_export]
macro_rules! select {
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        __crossbeam_channel_parse!(
            __crossbeam_channel_codegen
            $($case $(($($args)*))* => $body,)*
        )
    };

    ($($tokens:tt)*) => {
        __crossbeam_channel_parse!(
            __crossbeam_channel_codegen
            $($tokens)*
        )
    };
}

// TODO: explain
// loop { try; register; is_blocked; unregister; accept; write/read }

#[derive(Default)]
pub struct Token {
    pub after: flavors::after::AfterToken,
    pub array: flavors::array::ArrayToken,
    pub list: flavors::list::ListToken,
    pub tick: flavors::tick::TickToken,
    pub zero: flavors::zero::ZeroToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    Waiting,
    Aborted,
    Closed,
    Selected(usize),
}

impl Select {
    #[inline]
    pub fn hook<T>(r: &mut T) -> Select {
        Select::Selected(r as *mut T as usize)
    }
}

impl From<usize> for Select {
    #[inline]
    fn from(id: usize) -> Select {
        match id {
            0 => Select::Waiting,
            1 => Select::Aborted,
            2 => Select::Closed,
            id => Select::Selected(id),
        }
    }
}

impl Into<usize> for Select {
    #[inline]
    fn into(self) -> usize {
        match self {
            Select::Waiting => 0,
            Select::Aborted => 1,
            Select::Closed => 2,
            Select::Selected(id) => id,
        }
    }
}

pub trait SelectHandle {
    fn try(&self, token: &mut Token) -> bool;

    fn retry(&self, token: &mut Token) -> bool;

    fn deadline(&self) -> Option<Instant>;

    fn register(&self, token: &mut Token, select: Select) -> bool;

    fn unregister(&self, select: Select);

    fn accept(&self, token: &mut Token) -> bool;
}

impl<'a, T: SelectHandle> SelectHandle for &'a T {
    fn try(&self, token: &mut Token) -> bool {
        (**self).try(token)
    }

    fn retry(&self, token: &mut Token) -> bool {
        (**self).retry(token)
    }

    fn deadline(&self) -> Option<Instant> {
        (**self).deadline()
    }

    fn register(&self, token: &mut Token, select: Select) -> bool {
        (**self).register(token, select)
    }

    fn unregister(&self, select: Select) {
        (**self).unregister(select);
    }

    fn accept(&self, token: &mut Token) -> bool {
        (**self).accept(token)
    }
}
