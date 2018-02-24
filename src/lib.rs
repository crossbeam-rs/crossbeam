extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;
#[macro_use]
extern crate scopeguard;

mod base;
pub mod map;
pub mod set;

pub use map::SkipMap;
pub use set::SkipSet;
