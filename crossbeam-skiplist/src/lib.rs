//! TODO

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
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
extern crate crossbeam_utils as utils;
extern crate scopeguard;

pub mod base;
#[doc(inline)]
pub use crate::base::SkipList;

cfg_if! {
    if #[cfg(feature = "std")] {
        pub mod map;
        #[doc(inline)]
        pub use crate::map::SkipMap;

        pub mod set;
        #[doc(inline)]
        pub use crate::set::SkipSet;
    }
}
