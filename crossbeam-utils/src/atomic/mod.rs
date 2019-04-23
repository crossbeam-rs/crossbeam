//! Atomic types.

cfg_if! {
    // TODO(jeehoonkang): want to express `target_pointer_width >= "64"`, but it's not expressible
    // in Rust for the time being.
    if #[cfg(target_pointer_width = "64")] {
        mod seq_lock;
    } else {
        #[path = "seq_lock_wide.rs"]
        mod seq_lock;
    }
}

mod atomic_cell;
mod consume;

pub use self::atomic_cell::AtomicCell;
pub use self::consume::AtomicConsume;
