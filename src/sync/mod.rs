//! Synchronization primitives.

mod ms_queue;
mod seg_queue;
mod treiber_stack;
mod arc_cell;

/// Multi-producer multi-consumer channels for message passing.
///
/// See [the `crossbeam-channel` crate](https://github.com/crossbeam-rs/crossbeam-channel) for more
/// information.
///
/// Here's a test code that checks if the `select!` macro is re-exported:
///
/// ```
/// #[macro_use]
/// extern crate crossbeam;
/// fn main() {
/// use std::thread;
/// use crossbeam::sync::channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// thread::spawn(move || s1.send("foo"));
/// thread::spawn(move || s2.send("bar"));
///
/// // Only one of these two receive operations will be executed.
/// select! {
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
///     recv(r2, msg) => assert_eq!(msg, Some("bar")),
/// }
/// }
/// ```
pub mod channel {
    pub use crossbeam_channel::*;
}

/// A concurrent work-stealing deque.
///
/// See [the `crossbeam-deque` crate](https://github.com/crossbeam-rs/crossbeam-deque) for more
/// information.
pub mod deque {
    pub use crossbeam_deque::*;
}

pub use self::ms_queue::MsQueue;
pub use self::seg_queue::SegQueue;
pub use self::treiber_stack::TreiberStack;
pub use self::arc_cell::ArcCell;
