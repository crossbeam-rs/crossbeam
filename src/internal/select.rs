use std::time::Instant;

use flavors;

#[derive(Default)]
pub struct Token {
    pub after: flavors::after::AfterToken,
    pub array: flavors::array::ArrayToken,
    pub list: flavors::list::ListToken,
    pub tick: flavors::tick::TickToken,
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
    fn try(&self, token: &mut Token) -> bool;

    fn retry(&self, token: &mut Token) -> bool;

    fn deadline(&self) -> Option<Instant>;

    fn register(&self, token: &mut Token, case_id: CaseId) -> bool;

    fn unregister(&self, case_id: CaseId);

    fn accept(&self, token: &mut Token) -> bool;
}

impl<'a, T: Select> Select for &'a T {
    fn try(&self, token: &mut Token) -> bool {
        (**self).try(token)
    }

    fn retry(&self, token: &mut Token) -> bool {
        (**self).retry(token)
    }

    fn deadline(&self) -> Option<Instant> {
        (**self).deadline()
    }

    fn register(&self, token: &mut Token, case_id: CaseId) -> bool {
        (**self).register(token, case_id)
    }

    fn unregister(&self, case_id: CaseId) {
        (**self).unregister(case_id);
    }

    fn accept(&self, token: &mut Token) -> bool {
        (**self).accept(token)
    }
}

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
///
/// TODO: explain that select is slower than recv()/send()/try_recv()
/// TODO: explain that senders/receivers are first evaluated, and one message only on success
/// TODO: explain that panicking message computation will abort
///
/// TODO: document that we can't expect a mut iterator because of Clone requirement
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
