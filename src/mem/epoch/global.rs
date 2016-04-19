// Definition of global epoch state. The `get` function is the way to
// access this data externally (until const fn is stabilized...).

use std::sync::atomic::AtomicUsize;

use mem::CachePadded;
use mem::epoch::garbage;
use mem::epoch::participants::Participants;

/// Global epoch state
#[derive(Debug)]
pub struct EpochState {
    /// Current global epoch
    pub epoch: CachePadded<AtomicUsize>,

    // FIXME: move this into the `garbage` module, rationalize API
    /// Global garbage bags
    pub garbage: [CachePadded<garbage::ConcBag>; 3],

    /// Participant list
    pub participants: Participants,
}

unsafe impl Send for EpochState {}
unsafe impl Sync for EpochState {}

pub use self::imp::get;

#[cfg(not(feature = "nightly"))]
mod imp {
    use std::mem;
    use std::sync::atomic::{self, AtomicUsize};
    use std::sync::atomic::Ordering::Relaxed;

    use super::EpochState;
    use mem::CachePadded;
    use mem::epoch::participants::Participants;

    impl EpochState {
        fn new() -> EpochState {
            EpochState {
                epoch: CachePadded::zeroed(),
                garbage: [CachePadded::zeroed(),
                          CachePadded::zeroed(),
                          CachePadded::zeroed()],
                participants: Participants::new(),
            }
        }
    }

    static EPOCH: AtomicUsize = atomic::ATOMIC_USIZE_INIT;

    pub fn get() -> &'static EpochState {
        let mut addr = EPOCH.load(Relaxed);

        if addr == 0 {
            let boxed = Box::new(EpochState::new());
            let raw = Box::into_raw(boxed);

            addr = EPOCH.compare_and_swap(0, raw as usize, Relaxed);
            if addr != 0 {
                let boxed = unsafe { Box::from_raw(raw) };
                mem::drop(boxed);
            } else {
                addr = raw as usize;
            }
        }

        unsafe {
            &*(addr as *mut EpochState)
        }
    }
}

#[cfg(feature = "nightly")]
mod imp {
    use super::EpochState;
    use mem::CachePadded;
    use mem::epoch::participants::Participants;

    impl EpochState {
        const fn new() -> EpochState {
            EpochState {
                epoch: CachePadded::zeroed(),
                garbage: [CachePadded::zeroed(),
                          CachePadded::zeroed(),
                          CachePadded::zeroed()],
                participants: Participants::new(),
            }
        }
    }

    static EPOCH: EpochState = EpochState::new();
    pub fn get() -> &'static EpochState { &EPOCH }
}
