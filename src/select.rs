use std::time::{Duration, Instant};

use channel::{Receiver, Sender};
use flavors;
use utils::Backoff;

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
    fn promise(&self, token: &mut Token, case_id: CaseId);
    fn is_blocked(&self) -> bool;
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
    fn to_receivers(&'a self) -> Self::Iter;
}

impl<'a, T> RecvArgument<'a, T> for &'a Receiver<T> {
    type Iter = ::std::option::IntoIter<&'a Receiver<T>>;
    fn to_receivers(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Receiver<T>> + Clone> RecvArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;
    fn to_receivers(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

pub trait SendArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Sender<T>>;
    fn to_senders(&'a self) -> Self::Iter;
}

impl<'a, T> SendArgument<'a, T> for &'a Sender<T> {
    type Iter = ::std::option::IntoIter<&'a Sender<T>>;
    fn to_senders(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Sender<T>> + Clone> SendArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;
    fn to_senders(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

pub trait DefaultArgument {
    fn to_instant(self) -> Option<Instant>;
}

impl DefaultArgument for Instant {
    fn to_instant(self) -> Option<Instant> {
        Some(self)
    }
}

impl DefaultArgument for Option<Instant> {
    fn to_instant(self) -> Option<Instant> {
        self
    }
}

impl DefaultArgument for Duration {
    fn to_instant(self) -> Option<Instant> {
        Some(Instant::now() + self)
    }
}

impl DefaultArgument for Option<Duration> {
    fn to_instant(self) -> Option<Instant> {
        self.map(|duration| Instant::now() + duration)
    }
}

impl<'a, T: DefaultArgument + Clone> DefaultArgument for &'a T {
    fn to_instant(self) -> Option<Instant> {
        DefaultArgument::to_instant(self.clone())
    }
}
