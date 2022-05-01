//! Miscellaneous utilities.

#[cfg(not(crossbeam_no_atomic_64))]
pub(crate) use core::sync::atomic::AtomicU64;

#[cfg(crossbeam_no_atomic_64)]
use core::sync::atomic::Ordering;

#[cfg(crossbeam_no_atomic_64)]
#[derive(Debug)]
#[repr(transparent)]
pub(crate) struct AtomicU64 {
    inner: crossbeam_utils::atomic::AtomicCell<u64>,
}

#[cfg(crossbeam_no_atomic_64)]
impl AtomicU64 {
    pub(crate) const fn new(v: u64) -> Self {
        Self {
            inner: crossbeam_utils::atomic::AtomicCell::new(v),
        }
    }
    pub(crate) fn load(&self, _order: Ordering) -> u64 {
        self.inner.load()
    }
    pub(crate) fn store(&self, val: u64, _order: Ordering) {
        self.inner.store(val);
    }
    pub(crate) fn compare_exchange_weak(
        &self,
        current: u64,
        new: u64,
        _success: Ordering,
        _failure: Ordering,
    ) -> Result<u64, u64> {
        self.inner.compare_exchange(current, new)
    }
}
