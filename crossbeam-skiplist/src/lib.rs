//! TODO

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

#[macro_use]
extern crate cfg_if;

#[cfg_attr(
    feature = "nightly",
    cfg(all(target_has_atomic = "cas", target_has_atomic = "ptr"))
)]
cfg_if! {
    if #[cfg(any(feature = "alloc", feature = "std"))] {
        extern crate alloc;

        pub mod base;
        #[doc(inline)]
        pub use crate::base::SkipList;
    }
}

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
