//! Wrappers around the channel interface for exercising code paths more thoroughly.
//!
//! There's a lot of code duplication in the channel implementation. For example, which code gets
//! executed when receiving a message depends on many factors:
//!
//! - Which channel flavor is used.
//! - Whether the receiver was ever cloned (in after and tick flavors).
//! - Whether the operation is non-blocking, blocking, or blocking-with-timeout.
//! - Whether it is a method called on `Receiver` or on `Select`.
//!
//! These wrappers are used to force the tests to take a different code path from the default one,
//! which makes for a much higher code coverage.

#![allow(dead_code)]

pub mod cloned;
pub mod select;
pub mod vanilla;
