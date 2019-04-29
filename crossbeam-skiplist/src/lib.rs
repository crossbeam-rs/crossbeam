//! TODO

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![cfg_attr(not(feature = "std"), no_std)]

#[macro_use]
extern crate cfg_if;
#[cfg(feature = "std")]
extern crate core;

cfg_if! {
    if #[cfg(feature = "nightly")] {
        extern crate alloc;
    } else {
        #[cfg(feature = "std")]
        extern crate std as alloc;
    }
}

cfg_if! {
    if #[cfg(any(feature = "std", feature = "nightly"))] {
        extern crate crossbeam_epoch as epoch;
        extern crate crossbeam_utils as utils;
        extern crate scopeguard;

        pub mod base;
        #[doc(inline)]
        pub use base::SkipList;
    }
}

cfg_if! {
    if #[cfg(feature = "std")] {
        pub mod map;
        #[doc(inline)]
        pub use map::SkipMap;

        pub mod set;
        #[doc(inline)]
        pub use set::SkipSet;
    }
}
