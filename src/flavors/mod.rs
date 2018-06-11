//! Channel flavors.
//!
//! There are five flavors:
//!
//! 1. `after` - Channel that delivers a message after a certain amount of time.
//! 2. `array` - Bounded channel based on a preallocated array.
//! 3. `list` - Unbounded channel implemented as a linked list.
//! 4. `tick` - Channel that delivers messages periodically.
//! 5. `zero` - Zero-capacity channel.

pub mod after;
pub mod array;
pub mod list;
pub mod tick;
pub mod zero;
