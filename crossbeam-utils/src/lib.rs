//! Utilities for concurrent programming.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(alloc))]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]
#![cfg_attr(feature = "nightly", feature(integer_atomics))]

#[macro_use]
extern crate cfg_if;
#[cfg(feature = "std")]
extern crate core;

cfg_if! {
    if #[cfg(feature = "nightly")] {
        extern crate alloc;
    } else {
        mod alloc {
            extern crate std;
            pub use self::std::*;
        }
    }
}

pub mod atomic;

mod cache_padded;
pub use cache_padded::CachePadded;

cfg_if! {
    if #[cfg(feature = "std")] {
        pub mod sync;
        pub mod thread;
    }
}
