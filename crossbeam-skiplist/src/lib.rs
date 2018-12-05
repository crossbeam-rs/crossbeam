// #![warn(missing_docs)] // TODO: Uncomment this.
// #![warn(missing_debug_implementations)] // TODO: Uncomment this.
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(alloc))]

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

extern crate crossbeam_epoch as epoch;
extern crate scopeguard;

pub mod base;
pub use base::SkipList;

cfg_if! {
    if #[cfg(feature = "std")] {
        pub mod map;
        pub use map::SkipMap;

        pub mod set;
        pub use set::SkipSet;
    }
}
