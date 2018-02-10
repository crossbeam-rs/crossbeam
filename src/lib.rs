//! Epoch-based memory reclamation.
//!
//! An interesting problem concurrent collections deal with comes from the remove operation.
//! Suppose that a thread removes an element from a lock-free map, while another thread is reading
//! that same element at the same time. The first thread must wait until the second thread stops
//! reading the element. Only then it is safe to destruct it.
//!
//! Programming languages that come with garbage collectors solve this problem trivially. The
//! garbage collector will destruct the removed element when no thread can hold a reference to it
//! anymore.
//!
//! This crate implements a basic memory reclamation mechanism, which is based on epochs. When an
//! element gets removed from a concurrent collection, it is inserted into a pile of garbage and
//! marked with the current epoch. Every time a thread accesses a collection, it checks the current
//! epoch, attempts to increment it, and destructs some garbage that became so old that no thread
//! can be referencing it anymore.
//!
//! That is the general mechanism behind epoch-based memory reclamation, but the details are a bit
//! more complicated. Anyhow, memory reclamation is designed to be fully automatic and something
//! users of concurrent collections don't have to worry much about.
//!
//! # Pointers
//!
//! Concurrent collections are built using atomic pointers. This module provides [`Atomic`], which
//! is just a shared atomic pointer to a heap-allocated object. Loading an [`Atomic`] yields a
//! [`Shared`], which is an epoch-protected pointer through which the loaded object can be safely
//! read.
//!
//! # Pinning
//!
//! Before an [`Atomic`] can be loaded, a participant must be [`pin`]ned. By pinning a participant
//! we declare that any object that gets removed from now on must not be destructed just
//! yet. Garbage collection of newly removed objects is suspended until the participant gets
//! unpinned.
//!
//! # Garbage
//!
//! Objects that get removed from concurrent collections must be stashed away until all currently
//! pinned participants get unpinned. Such objects can be stored into a [`Garbage`], where they are
//! kept until the right time for their destruction comes.
//!
//! There is a global shared instance of garbage queue. You can [`defer`] the execution of an
//! arbitrary function until the global epoch is advanced enough. Most notably, concurrent data
//! structures may defer the deallocation of an object.
//!
//! # APIs
//!
//! For majority of use cases, just use the default garbage collector by invoking [`pin`]. If you
//! want to create your own garbage collector, use the [`Collector`] API.
//!
//! [`Atomic`]: struct.Atomic.html
//! [`Collector`]: struct.Collector.html
//! [`Shared`]: struct.Shared.html
//! [`pin`]: fn.pin.html
//! [`defer`]: fn.defer.html

#![cfg_attr(feature = "nightly", feature(const_fn))]
#![cfg_attr(feature = "nightly", feature(alloc))]
#![cfg_attr(not(test), no_std)]

#[cfg(all(not(test), feature = "use_std"))]
#[macro_use]
extern crate std;
#[cfg(test)]
extern crate core;

// Use liballoc on nightly to avoid a dependency on libstd
#[cfg(feature = "nightly")]
extern crate alloc;
#[cfg(not(feature = "nightly"))]
mod alloc {
    // Tweak the module layout to match the one in liballoc
    extern crate std;
    pub use self::std::boxed;
    pub use self::std::sync as arc;
}

#[cfg(feature = "manually_drop")]
mod nodrop {
    pub use std::mem::ManuallyDrop as NoDrop;
}
#[cfg(not(feature = "manually_drop"))]
extern crate nodrop;

extern crate arrayvec;
extern crate crossbeam_utils;
#[cfg(feature = "use_std")]
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate memoffset;
#[macro_use]
extern crate scopeguard;

mod atomic;
mod collector;
#[cfg(feature = "use_std")]
mod default;
mod deferred;
mod epoch;
mod garbage;
mod guard;
mod internal;
mod sync;

pub use self::atomic::{Atomic, CompareAndSetError, CompareAndSetOrdering, Owned, Shared};
pub use self::guard::{unprotected, Guard};
#[cfg(feature = "use_std")]
pub use self::default::{default_handle, is_pinned, pin};
pub use self::collector::{Collector, Handle};
