//! TODO

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![cfg_attr(feature = "no_std", no_std)]

#[macro_use]
extern crate cfg_if;
#[cfg(not(feature = "no_std"))]
extern crate core;

cfg_if! {
    if #[cfg(feature = "no_std")] {
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
pub use base::SkipList;

cfg_if! {
    if #[cfg(not(feature = "no_std"))] {
        pub mod map;
        #[doc(inline)]
        pub use map::SkipMap;

        pub mod set;
        #[doc(inline)]
        pub use set::SkipSet;
    }
}
