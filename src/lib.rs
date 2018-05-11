#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic, integer_atomics))]
#![cfg_attr(not(feature = "use_std"), no_std)]

#[cfg(feature = "use_std")]
extern crate core;

pub mod cache_padded;
#[cfg(feature = "use_std")]
pub mod scoped;
pub mod consume;
