//! Epoch-based memory reclamation.
//!
//! # Pointers
//!
//! Concurrent collections are built using atomic pointers. This module provides [`Atomic`], which
//! is just a shared atomic pointer to a heap-allocated object. Loading an [`Atomic`] yields a
//! [`Ptr`], which is an epoch-protected pointer through which the loaded object can be safely read.
//!
//! # Pinning
//!
//! Before an [`Atomic`] can be loaded, the current mutator must be [`pin`]ned. By pinning a mutator
//! we declare that any object that gets removed from now on must not be destructed just
//! yet. Garbage collection of newly removed objects is suspended until the mutator gets unpinned.
//!
//! # Garbage
//!
//! Objects that get removed from concurrent collections must be stashed away until all currently
//! pinned mutators get unpinned. Such objects can be stored into a [`Garbage`], where they are kept
//! until the right time for their destruction comes.
//!
//! There is a global shared instance of garbage queue, which can deallocate ([`defer_free`]) or
//! drop ([`defer_drop`]) objects, or even run arbitrary destruction procedures ([`defer`]).
//!
//! [`Atomic`]: struct.Atomic.html
//! [`Ptr`]: struct.Ptr.html
//! [`pin`]: fn.pin.html
//! [`defer_free`]: fn.defer_free.html
//! [`defer_drop`]: fn.defer_drop.html
//! [`defer`]: fn.defer.html

#![cfg_attr(feature = "nightly", feature(const_fn))]

#[macro_use(defer)]
extern crate scopeguard;
#[macro_use]
extern crate lazy_static;
extern crate arrayvec;
extern crate boxfnonce;
extern crate crossbeam_utils;

mod atomic;
mod mutator;
mod garbage;
mod epoch;
mod global;
mod sync;

pub use self::atomic::{Atomic, CompareAndSetOrdering, Owned, Ptr};
pub use self::mutator::{Scope, unprotected};
pub use self::global::{pin, is_pinned};
