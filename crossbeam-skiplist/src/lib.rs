//! TODO

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(cfg_target_has_atomic))]

#[macro_use]
extern crate cfg_if;

cfg_if! {
    if #[cfg(all(feature = "alloc", feature = "nightly"))] {
        extern crate alloc;
    } else if #[cfg(feature = "std")] {
        extern crate std as alloc;
    }
}

cfg_if! {
    if #[cfg(any(all(feature = "alloc", feature = "nightly"), feature = "std"))] {
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
