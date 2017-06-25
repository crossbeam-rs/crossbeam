#![cfg_attr(feature = "nightly", feature(const_fn))]

mod atomic;

pub use atomic::{Atomic, CompareAndSetOrdering, Owned, Ptr};

pub struct Scope {
    _private: (),
}

pub fn pin<F, T>(f: F) -> T
where
    F: FnOnce(&Scope) -> T
{
    // TODO: Implement actual pinning.

    let scope = &Scope { _private: () };
    f(scope)
}
