//! Channel flavors.
//!
//! There are five flavors:
//!
//! 1. `after` - A channel that delivers a message after a certain amount of time.
//! 2. `array` - A bounded channel based on a preallocated array.
//! 3. `list` - An unbounded channel implemented as a linked list.
//! 4. `tick` - A channel that delivers messages periodically.
//! 5. `zero` - A zero-capacity channel, or sometimes called *rendezvous* channel.

pub mod after;
pub mod array;
pub mod list;
pub mod tick;
pub mod zero;
