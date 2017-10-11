//! There are three flavors:
//!
//! 1. `array`: bounded channel that uses a preallocated array
//! 2. `list`: unbounded channel implemented as a linked list
//! 3. `zero`: zero-capacity channel, or sometimes called *rendezvous* channel

pub mod array;
pub mod list;
pub mod zero;
