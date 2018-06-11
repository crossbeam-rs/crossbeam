//! Interface to the select mechanism.

use std::time::Instant;

use flavors;

/// Waits on a set of channel operations.
///
/// This macro allows declaring a set of channel operations and blocking until any one of them
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
/// // Since both operations are initially ready, a random one will be executed.
/// select! {
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
///     send(s2, "bar") => assert_eq!(r2.recv(), Some("bar")),
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
/// // Both receivers are initially ready so one of the two receive operations
/// // will be chosen randomly.
/// select! {
///     // The third argument to `recv` is optional and is assigned a
///     // reference to the receiver the message was received from.
///     recv(receivers, msg, from) => {
///         for (i, r) in receivers.iter().enumerate() {
///             if r == from {
///                 println!("Received {:?} from the {}-th receiver.", msg, i);
///             }
///         }
///     }
/// }
/// # }
/// ```
///
/// # Syntax
///
/// An invocation of `select!` consists of a list of cases. Consecutive cases are delimited by a
/// comma, but it's not required if the preceding case has a block expression (the syntax is very
/// similar to `match` statements).
///
/// The following invocation illustrates all the possible forms cases can take:
///
/// ```ignore
/// select! {
///     recv(r1) => body1,
///     recv(r2, msg2) => body2,
///     recv(r3, msg3, from3) => body3,
///
///     send(s4, msg4) => body4,
///     send(s5, msg5, into5) => body5,
///
///     default => body6,
/// }
/// ```
///
/// Input expressions: `r1`, `r2`, `r3`, `s4`, `s5`, `msg4`, `msg5`, `body1`, `body2`, `body3`,
/// `body4`, `body5`, `body6`
///
/// Output patterns: `msg2`, `msg3`, `msg4`, `msg5`, `from3`, `into5`
///
/// Types of expressions and patterns (generic over types `A`, `B`, `C`, `D`, `E`, and `F`):
///
/// * `r1`: one of `Receiver<A>`, `&Receiver<A>`, or `impl IntoIterator<Item = &Receiver<A>>`
/// * `r2`: one of `Receiver<B>`, `&Receiver<B>`, or `impl IntoIterator<Item = &Receiver<B>>`
/// * `r3`: one of `Receiver<C>`, `&Receiver<C>`, or `impl IntoIterator<Item = &Receiver<C>>`
/// * `s4`: one of `Sender<D>`, `&Sender<D>`, or `impl IntoIterator<Item = &Sender<D>>`
/// * `s5`: one of `Sender<E>`, `&Sender<E>`, or `impl IntoIterator<Item = &Sender<E>>`
/// * `msg2`: `Option<B>`
/// * `msg3`: `Option<C>`
/// * `msg4`: `D`
/// * `msg5`: `E`
/// * `from3`: `&Receiver<C>`
/// * `into5`: `&Sender<E>`
/// * `body1`, `body2`, `body3`, `body4`, `body5`, `body6`: `F`
///
/// Pattern `from3` is bound to the receiver in `r3` from which `msg3` was received.
///
/// Pattern `into5` is bound to the sender in `s5` into which `msg5` was sent.
///
/// There can be at most one `default` case.
///
/// # Execution
///
/// 1. All sender and receiver arguments (`r1`, `r2`, `r3`, `s4`, and `s5`) are evaluated.
/// 2. If any of the `recv` or `send` operations are ready, one of them is executed. If multiple
///    operations are ready, a random one is chosen.
/// 3. If none of the `recv` and `send` operations are ready, the `default` case is executed. If
///    there is no `default` case, the current thread is blocked until an operation becomes ready.
/// 4. If a `recv` operation gets executed, the message pattern (`msg2` or `msg3`) is
///    bound to the received message, and the receiver pattern (`from3`) is bound to the receiver
///    from which the message was received.
/// 5. If a `send` operation gets executed, the message (`msg4` or `msg5`) is evaluated and sent
///    into the channel. Then, the sender pattern (`into5`) is bound to the sender into which the
///    message was sent.
/// 6. Finally, the body (`body1`, `body2`, `body3`, `body4`, `body5`, or `body6`) of the executed
///    case is evaluated. The whole `select!` invocation evaluates to that expression.
///
/// **Note**: If evaluation of `msg4` or `msg5` panics, the process will be aborted because it's
/// impossible to recover from such panics. All the other expressions are allowed to panic,
/// however.
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

/// Temporary data that gets initialized during select or a blocking operation, and is consumed by
/// `read` or `write`.
///
/// Each field contains data associated with a specific channel flavor.
#[derive(Default)]
pub struct Token {
    pub after: flavors::after::AfterToken,
    pub array: flavors::array::ArrayToken,
    pub list: flavors::list::ListToken,
    pub tick: flavors::tick::TickToken,
    pub zero: flavors::zero::ZeroToken,
}

/// Identifier associated with an operation by a specific thread on a specific channel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Operation(usize);

impl Operation {
    /// Creates an identifier from a mutable reference.
    ///
    /// This function essentially just turns the address of the reference into a number. The
    /// reference should point to a variable that is specific to the thread and the operation,
    /// and is alive for the entire duration of select or blocking operation.
    #[inline]
    pub fn hook<T>(r: &mut T) -> Operation {
        Operation(r as *mut T as usize)
    }
}

/// Current state of a select or a blocking operation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Select {
    /// Still waiting for an operation.
    Waiting,

    /// The select or blocking operation has been aborted.
    Aborted,

    /// A channel was closed.
    Closed,

    /// An operation became ready.
    Operation(Operation),
}

impl From<usize> for Select {
    #[inline]
    fn from(val: usize) -> Select {
        match val {
            0 => Select::Waiting,
            1 => Select::Aborted,
            2 => Select::Closed,
            oper => Select::Operation(Operation(oper)),
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
            Select::Operation(Operation(val)) => val,
        }
    }
}

/// A receiver or a sender that can participate in select.
///
/// This is a handle that assists select in executing the operation, registration, deciding on the
/// appropriate deadline for blocking, etc.
pub trait SelectHandle {
    /// Attempts to execute the operation and returns `true` on success.
    fn try(&self, token: &mut Token) -> bool;

    /// Attempts to execute the operation again and returns `true` on success.
    ///
    /// Retries are allowed to take a little bit more time than the initial try.
    fn retry(&self, token: &mut Token) -> bool;

    /// Returns a deadline for the operation, if there is one.
    fn deadline(&self) -> Option<Instant>;

    /// Registers the operation.
    fn register(&self, token: &mut Token, oper: Operation) -> bool;

    /// Unregisters the operation.
    fn unregister(&self, oper: Operation);

    /// Attempts to execute the selected operation.
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

    fn register(&self, token: &mut Token, oper: Operation) -> bool {
        (**self).register(token, oper)
    }

    fn unregister(&self, oper: Operation) {
        (**self).unregister(oper);
    }

    fn accept(&self, token: &mut Token) -> bool {
        (**self).accept(token)
    }
}
