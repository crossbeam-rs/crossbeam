//! Synchronization primitives.

pub use self::ms_queue::MsQueue;
pub use utils::atomic_option::AtomicOption;
pub use self::treiber_stack::TreiberStack;
pub use self::seg_queue::SegQueue;
pub use self::arc_cell::ArcCell;

pub extern crate crossbeam_deque as deque;

mod ms_queue;
mod seg_queue;
mod treiber_stack;
mod arc_cell;
