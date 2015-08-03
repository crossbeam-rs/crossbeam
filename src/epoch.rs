use std::any::Any;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};
use std::marker;
use std::ops::{Deref, DerefMut};

use bag::Bag;
use cache_padded::CachePadded;

pub struct Participants {
    bag: Bag<CachePadded<Participant>>,
}

struct Participant {
    epoch: AtomicUsize,
    in_critical: AtomicBool,
    ref_count: AtomicUsize,
}

impl Participants {
    const fn new() -> Participants {
        Participants { bag: Bag::new() }
    }

    fn enroll(&self) -> *const Participant {
        let participant = Participant {
            epoch: AtomicUsize::new(0),
            in_critical: AtomicBool::new(false),
            ref_count: AtomicUsize::new(1)
        };
        unsafe {
            (*self.bag.insert(CachePadded::new(participant))).deref()
        }
    }
}

struct EpochState {
    epoch: CachePadded<AtomicUsize>,
    garbage: [CachePadded<Bag<*mut Any>>; 3],
    participants: Participants,
}

unsafe impl Send for EpochState {}
unsafe impl Sync for EpochState {}

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

struct Handle {
    participant: *const Participant,
    op_count: u32,
}

impl Handle {
    fn enter(&mut self) {
        let part = unsafe { &*self.participant };
        part.in_critical.store(true, Relaxed);
        atomic::fence(SeqCst);

        let epoch = EPOCH.epoch.load(Relaxed);
        if epoch == part.epoch.load(Relaxed) {
            self.op_count = self.op_count.saturating_add(1);
            self.collect()
        } else {
            part.epoch.store(epoch, Relaxed);
            self.op_count = 0
        }
    }

    fn exit(&mut self) {
        unsafe {
            (*self.participant).in_critical.store(false, Release);
        }
    }

    fn reclaim<T: Any>(&mut self, data: *mut T) {
        unsafe {
            EPOCH.garbage[(*self.participant).epoch.load(Relaxed)].insert(data);
        }
    }

    fn collect(&mut self) {
        let cur_epoch = EPOCH.epoch.load(SeqCst);

        for p in EPOCH.participants.bag.iter() {
            if p.in_critical.load(Relaxed) && p.epoch.load(Relaxed) != cur_epoch { return }
        }

        let new_epoch = (cur_epoch + 1) % 3;
        atomic::fence(Acquire);
        if EPOCH.epoch.compare_and_swap(cur_epoch, new_epoch, SeqCst) != cur_epoch { return }

        unsafe {
            for g in EPOCH.garbage[(new_epoch + 1) % 3].iter_clobber() {
                // the pointer g is now unique; drop it
                mem::drop(Box::from_raw(g))
            }
        }
    }
}
