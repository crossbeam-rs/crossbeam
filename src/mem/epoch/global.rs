//! The global epoch state.
//!
//! The `get` function is the way to access this data externally (until const fn is stabilized...).

use std::sync::atomic::AtomicUsize;

use mem::CachePadded;
use mem::epoch::garbage;
use mem::epoch::participants::Participants;

/// The global epoch state.
///
/// The epoch state has a thread-local and a global part. This is the global part.
///
/// The global state is shared among all threads, who we modify it once in a while, propagating
/// their local state into the gloval state.
#[derive(Debug, Default)]
pub struct EpochState {
    /// Current global epoch.
    ///
    /// Upon a new epoch, this is incremented.
    // FIXME: In theory this could overflow eventually.
    pub epoch: CachePadded<AtomicUsize>,
    /// The set of garbage to be destroyed.
    pub garbage: garbage::Global,
    /// Participant list.
    ///
    /// This list tracks the thread currently "participating" in the global epoch.
    pub participants: Participants,
}

/// The global epoch.
#[cfg(feature = "nightly")]
static EPOCH: EpochState = EpochState {
    epoch: CachePadded::new(),
    garbage: garbage::Global::new(),
    participants: Participants::new(),
};

// TODO: Remove when const fn is stabilized.
#[cfg(not(feature = "nightly"))]
lazy_static! {
    /// The global epoch.
    static EPOCH: EpochState = EpochState::default();
}

unsafe impl Send for EpochState {}
unsafe impl Sync for EpochState {}
