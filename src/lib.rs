#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic, integer_atomics))]
#![cfg_attr(not(feature = "use_std"), no_std)]

#[macro_use]
extern crate cfg_if;
#[cfg(feature = "use_std")]
extern crate core;

mod cache_padded;

pub mod atomic;
#[cfg(feature = "use_std")]
pub mod thread;

pub use cache_padded::CachePadded;
