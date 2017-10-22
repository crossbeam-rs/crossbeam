#![cfg_attr(feature = "nightly", feature(attr_literals, repr_align))]

#[macro_use]
extern crate cfg_if;

pub mod atomic_option;
pub mod cache_padded;
pub mod scoped;
