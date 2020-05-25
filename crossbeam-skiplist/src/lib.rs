//! TODO

#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms),
        allow(dead_code, unused_assignments, unused_variables)
    )
))]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

use cfg_if::cfg_if;

#[cfg_attr(feature = "nightly", cfg(target_has_atomic = "ptr"))]
cfg_if! {
    if #[cfg(feature = "alloc")] {
        extern crate alloc;

        use crossbeam_epoch as epoch;
        use crossbeam_utils as utils;

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
