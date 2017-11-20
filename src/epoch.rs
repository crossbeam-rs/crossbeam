//! The global epoch
//!
//! The last bit in this number is unused and is always zero. Every so often the global epoch is
//! incremented, i.e. we say it "advances". A pinned participant may advance the global epoch only
//! if all currently pinned participants have been pinned in the current epoch.
//!
//! If an object became garbage in some epoch, then we can be sure that after two advancements no
//! participant will hold a reference to it. That is the crux of safe memory reclamation.

use std::cmp;
use std::sync::atomic::{AtomicUsize, Ordering};

/// An epoch that can be marked as pinned or unpinned.
///
/// Internally, the epoch is represented as an integer that wraps around at some unspecified point
/// and a flag that represents whether it is pinned or unpinned.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub struct Epoch {
    /// The least significant bit is set if pinned. The rest of the bits hold the epoch.
    data: usize,
}

impl Epoch {
    /// Returns the starting epoch in unpinned state.
    #[inline]
    pub fn starting() -> Self {
        Self::default()
    }

    /// Returns the number of steps between two epochs.
    ///
    /// An epoch is internally represented as an integer that wraps around at some unspecified
    /// point. The returned distance between two epochs is the minimum number of steps required to
    /// go from `self` to `other` or go from `other` to `self`.
    #[inline]
    pub fn distance(&self, other: Self) -> usize {
        let (e1, _) = self.decompose();
        let (e2, _) = other.decompose();
        cmp::min(e1.wrapping_sub(e2), e2.wrapping_sub(e1))
    }

    /// Returns `true` if the epoch is marked as pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.decompose().1
    }

    /// Returns the same epoch, but marked as pinned.
    #[inline]
    pub fn pinned(&self) -> Epoch {
        Epoch {
            data: self.data | 1,
        }
    }

    /// Returns the same epoch, but marked as unpinned.
    #[inline]
    pub fn unpinned(&self) -> Epoch {
        Epoch {
            data: self.data & !1,
        }
    }

    /// Returns the successor epoch.
    ///
    /// The returned epoch will be marked as pinned only if the previous one was as well.
    #[inline]
    pub fn successor(&self) -> Epoch {
        Epoch {
            data: self.data.wrapping_add(2),
        }
    }

    /// Decomposes the internal data into the epoch and the pin state.
    #[inline]
    fn decompose(&self) -> (usize, bool) {
        (self.data >> 1, (self.data & 1) == 1)
    }
}

/// An atomic value that holds an `Epoch`.
#[derive(Default, Debug)]
pub struct AtomicEpoch {
    /// Since `Epoch` is just a wrapper around `usize`, an `AtomicEpoch` is similarly represented
    /// using an `AtomicUsize`.
    data: AtomicUsize,
}

impl AtomicEpoch {
    /// Creates a new atomic epoch.
    #[inline]
    pub fn new(epoch: Epoch) -> Self {
        let data = AtomicUsize::new(epoch.data);
        AtomicEpoch { data }
    }

    /// Loads a value from the atomic epoch.
    #[inline]
    pub fn load(&self, ord: Ordering) -> Epoch {
        Epoch {
            data: self.data.load(ord),
        }
    }

    /// Stores a value into the atomic epoch.
    #[inline]
    pub fn store(&self, epoch: Epoch, ord: Ordering) {
        self.data.store(epoch.data, ord);
    }

    /// Stores a value into the atomic epoch if the current value is the same as `current`.
    ///
    /// The return value is always the previous value. If it is equal to `current`, then the value
    /// is updated.
    ///
    /// The `Ordering` argument describes the memory ordering of this operation.
    #[inline]
    pub fn compare_and_swap(&self, current: Epoch, new: Epoch, ord: Ordering) -> Epoch {
        let data = self.data.compare_and_swap(current.data, new.data, ord);
        Epoch { data }
    }
}
