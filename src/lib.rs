#![cfg_attr(feature = "nightly", feature(attr_literals, repr_align))]
#![cfg_attr(not(feature = "use_std"), no_std)]

#[cfg(feature = "use_std")]
extern crate core;

#[macro_use]
extern crate cfg_if;

pub mod cache_padded;
#[cfg(feature = "use_std")]
pub mod atomic_option;
#[cfg(feature = "use_std")]
pub mod scoped;
