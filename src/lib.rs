#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic, integer_atomics))]
#![cfg_attr(not(feature = "use_std"), no_std)]

#[cfg(feature = "use_std")]
extern crate core;

mod cache_padded;
mod consume;

#[cfg(feature = "use_std")]
pub mod thread;

pub use cache_padded::CachePadded;
pub use consume::AtomicConsume;
