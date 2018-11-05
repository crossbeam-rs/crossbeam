#![cfg_attr(
    feature = "nightly",
    feature(cfg_target_has_atomic, integer_atomics)
)]
#![cfg_attr(not(feature = "std"), no_std)]

#[macro_use]
extern crate cfg_if;
#[cfg(feature = "std")]
extern crate core;
#[cfg(feature = "std")]
extern crate num_cpus;
#[cfg(feature = "std")]
extern crate parking_lot;
#[cfg(feature = "std")]
#[macro_use]
extern crate lazy_static;

mod cache_padded;

pub mod atomic;
#[cfg(feature = "std")]
pub mod sync;
#[cfg(feature = "std")]
pub mod thread;

pub use cache_padded::CachePadded;
