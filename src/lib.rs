#![cfg_attr(feature = "nightly", feature(alloc))]
#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate core;
#[cfg(all(not(test), feature = "use_std"))]
#[macro_use]
extern crate std;

// Use liballoc on nightly to avoid a dependency on libstd
#[cfg(feature = "nightly")]
extern crate alloc;
#[cfg(not(feature = "nightly"))]
mod alloc {
    // Tweak the module layout to match the one in liballoc
    extern crate std;
    pub use self::std::vec;
}

extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;
extern crate scopeguard;

pub mod base;
#[cfg(feature = "use_std")]
pub mod map;
#[cfg(feature = "use_std")]
pub mod set;

pub use base::SkipList;
#[cfg(feature = "use_std")]
pub use map::SkipMap;
#[cfg(feature = "use_std")]
pub use set::SkipSet;

/// An endpoint of a range of keys.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Bound<T> {
    /// An inclusive bound.
    Included(T),
    /// An exclusive bound.
    Excluded(T),
    /// An infinite endpoint. Indicates that there is no bound in this direction.
    Unbounded,
}
