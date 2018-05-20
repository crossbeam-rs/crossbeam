use std::time::{Duration, Instant};

use internal::channel::{Receiver, Sender};
use internal::utils::Backoff;
use flavors;

/// TODO
///
/// TODO: explain fairness, only one case fires
///
/// # Receiving
///
/// TODO receive from multiple channels
///
/// # Sending
///
/// TODO select with a receive and a send
///
/// # Default case
///
/// A special kind of case is `default`, which gets executed if none of the operations can be
/// executed (they would block):
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
///     recv(r, msg) => panic!(),
///     default => println!("The message is not yet available."),
/// }
///
/// // Block, but only for a certain amount of time:
/// let timeout = Duration::from_millis(500);
/// select! {
///     recv(r, msg) => panic!(),
///     default(timeout) => println!("Too late! The message is still not available."),
/// }
///
/// // Block, but only until a certain deadline:
/// let deadline = Instant::now() + Duration::from_secs(2);
/// select! {
///     recv(r, msg) => println!("Finally received: {:?}", msg),
///     default(deadline) => panic!(),
/// }
/// # }
/// ```
///
/// # Optional default case
///
/// TODO
///
/// # Iterators
///
/// TODO explain iterators and options, and how to capture the channel
///
/// # The full syntax
///
/// TODO: explain all cases, arguments, types
/// TODO: explain that there can be only one select
/// TODO: explain there must be at least one case (select { default => () } is permitted)
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

pub union Token {
    pub array: flavors::array::ArrayToken,
    pub list: flavors::list::ListToken,
    pub zero: flavors::zero::ZeroToken,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CaseId {
    pub id: usize,
}

impl CaseId {
    #[inline]
    pub fn none() -> Self {
        CaseId { id: 0 }
    }

    #[inline]
    pub fn abort() -> Self {
        CaseId { id: 1 }
    }

    #[inline]
    pub fn new(id: usize) -> Self {
        CaseId { id }
    }
}

impl From<usize> for CaseId {
    #[inline]
    fn from(id: usize) -> Self {
        CaseId { id }
    }
}

impl Into<usize> for CaseId {
    #[inline]
    fn into(self) -> usize {
        self.id
    }
}

pub trait Select {
    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool;

    // TODO: register
    fn promise(&self, token: &mut Token, case_id: CaseId);

    fn is_blocked(&self) -> bool;

    // TODO: unregister
    fn revoke(&self, case_id: CaseId);

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool;
}

impl<'a, T: Select> Select for &'a T {
    fn try(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        (**self).try(token, backoff)
    }

    fn promise(&self, token: &mut Token, case_id: CaseId) {
        (**self).promise(token, case_id);
    }

    fn is_blocked(&self) -> bool {
        (**self).is_blocked()
    }

    fn revoke(&self, case_id: CaseId) {
        (**self).revoke(case_id);
    }

    fn fulfill(&self, token: &mut Token, backoff: &mut Backoff) -> bool {
        (**self).fulfill(token, backoff)
    }
}

pub trait RecvArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Receiver<T>>;

    fn __as_recv_argument(&'a self) -> Self::Iter;
}

impl<'a, T> RecvArgument<'a, T> for &'a Receiver<T> {
    type Iter = ::std::option::IntoIter<&'a Receiver<T>>;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Receiver<T>> + Clone> RecvArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

pub trait SendArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Sender<T>>;

    fn __as_send_argument(&'a self) -> Self::Iter;
}

impl<'a, T> SendArgument<'a, T> for &'a Sender<T> {
    type Iter = ::std::option::IntoIter<&'a Sender<T>>;

    fn __as_send_argument(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Sender<T>> + Clone> SendArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;

    fn __as_send_argument(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

pub trait DefaultArgument {
    fn default_argument(self) -> Option<Instant>;
}

impl DefaultArgument for Instant {
    fn default_argument(self) -> Option<Instant> {
        Some(self)
    }
}

impl DefaultArgument for Option<Instant> {
    fn default_argument(self) -> Option<Instant> {
        self
    }
}

impl DefaultArgument for Duration {
    fn default_argument(self) -> Option<Instant> {
        Some(Instant::now() + self)
    }
}

impl DefaultArgument for Option<Duration> {
    fn default_argument(self) -> Option<Instant> {
        self.map(|duration| Instant::now() + duration)
    }
}

impl<'a, T: DefaultArgument + Clone> DefaultArgument for &'a T {
    fn default_argument(self) -> Option<Instant> {
        DefaultArgument::default_argument(self.clone())
    }
}
