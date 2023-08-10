use core::sync::atomic::Ordering;

use crate::primitive::sync::atomic::AtomicUsize;
use crossbeam_utils::CachePadded;

const STRIPES: usize = 16;
const HIGH_BIT: usize = !(usize::MAX >> 1);
const MAX_FAILED_RLOCKS: usize = HIGH_BIT + (HIGH_BIT >> 1);

#[derive(Default, Debug)]
/// Divided into a number of separate atomics to reduce read contention.
/// Uses the almost the same algorithm as atomic_refcell.
pub(crate) struct RefCount {
    counts: [CachePadded<AtomicUsize>; STRIPES],
}

impl RefCount {
    pub(crate) fn try_read(&self, hint: usize) -> bool {
        let state = self.counts[hint % STRIPES].fetch_add(1, Ordering::Acquire);
        if state < HIGH_BIT {
            return true;
        }
        if state > MAX_FAILED_RLOCKS {
            panic!("Too many failed pins");
        }
        return false;
    }
    pub(crate) fn done_read(&self, hint: usize) {
        self.counts[hint % STRIPES].fetch_sub(1, Ordering::Release);
    }

    pub(crate) fn try_write(&self) -> bool {
        if self.counts[0].load(Ordering::Relaxed) != 0 {
            return false;
        }
        for count in &self.counts[1..] {
            match count.load(Ordering::Acquire) {
                HIGH_BIT => {}
                0 if count
                    .compare_exchange(0, HIGH_BIT, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok() => {}
                _ => return false,
            }
        }
        return self.counts[0]
            .compare_exchange(0, HIGH_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok();
    }
    pub(crate) fn done_write(&self) {
        for count in &self.counts {
            count.store(0, Ordering::Release)
        }
    }
}
