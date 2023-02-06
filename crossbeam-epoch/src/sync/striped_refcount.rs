use core::sync::atomic::Ordering;

use crate::primitive::sync::atomic::AtomicUsize;
use crossbeam_utils::CachePadded;

const STRIPES: usize = 16;

#[derive(Default)]
/// A counter divided into a number of separate atomics to reduce contention.
pub(crate) struct StripedRefcount {
    counts: [CachePadded<AtomicUsize>; STRIPES],
}

impl StripedRefcount {
    /// Increment the counter. Performance will improve if `hint` is different on different cores.
    pub(crate) fn increment(&self, hint: usize, order: Ordering) {
        self.counts[hint % STRIPES].fetch_add(1, order);
    }

    /// Decrement the counter. Performance will improve if `hint` is different on different cores.
    pub(crate) fn decrement(&self, hint: usize, order: Ordering) {
        self.counts[hint % STRIPES].fetch_sub(1, order);
    }

    /// Read the counter's value. Note that this is not atomic.
    pub(crate) fn load(&self, order: Ordering) -> usize {
        self.counts
            .iter()
            .map(|c| c.load(order))
            .fold(0, |a, b| a.wrapping_add(b))
    }
}
