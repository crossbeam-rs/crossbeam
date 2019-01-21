//! Thread synchronization primitives.

mod parker;

pub use self::parker::{Parker, Unparker};
