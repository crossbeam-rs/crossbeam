use std::any::Any;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{self, Relaxed, Acquire, Release, SeqCst};
use std::ops::{Deref, DerefMut};
use std::marker;

use bag::Bag;
use cache_padded::CachePadded;

struct Participants {
    bag: Bag<CachePadded<Participant>>,
}

struct Participant {
    epoch: AtomicUsize,
    in_critical: AtomicBool,
}

impl Participants {
    const fn new() -> Participants {
        Participants { bag: Bag::new() }
    }

    fn enroll(&self) -> *const Participant {
        let participant = Participant {
            epoch: AtomicUsize::new(0),
            in_critical: AtomicBool::new(false),
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
}


pub fn try_collect() -> bool {
    let cur_epoch = EPOCH.epoch.load(SeqCst);

    for p in EPOCH.participants.bag.iter() {
        if p.in_critical.load(Relaxed) && p.epoch.load(Relaxed) != cur_epoch {
            return false
        }
    }

    let new_epoch = (cur_epoch + 1) % 3;
    atomic::fence(Acquire);
    if EPOCH.epoch.compare_and_swap(cur_epoch, new_epoch, SeqCst) != cur_epoch {
        return false
    }

    unsafe {
        for g in EPOCH.garbage[(new_epoch + 1) % 3].iter_clobber() {
            // the pointer g is now unique; drop it
            mem::drop(Box::from_raw(g))
        }
    }

    true
}

pub struct Owned<T> {
    data: Box<T>,
}

impl<T> Deref for Owned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T> DerefMut for Owned<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

#[derive(PartialEq, Eq)]
pub struct Shared<'a, T: 'a> {
    data: &'a T,
}

impl<'a, T> Copy for Shared<'a, T> {}
impl<'a, T> Clone for Shared<'a, T> {
    fn clone(&self) -> Shared<'a, T> {
        Shared { data: self.data }
    }
}

impl<'a, T> Deref for Shared<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

pub struct AtomicPtr<T> {
    ptr: atomic::AtomicPtr<T>,
}

impl<T> AtomicPtr<T> {

    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> Option<Shared<'a, T>> {
        let p = self.ptr.load(ord);
        if p == ptr::null_mut() {
            None
        } else {
            Some(Shared {
                data: unsafe { mem::transmute(p) },
            })
        }
    }

    pub fn store<'a>(&self, val: Owned<T>, ord: Ordering, g: &'a Guard) -> Shared<'a, T> {
        unsafe {
            let shared = Shared { data: mem::transmute(val.deref()) };
            self.store_shared(shared, ord);
            shared
        }
    }

    pub fn store_null(&self, ord: Ordering) {
        self.ptr.store(ptr::null_mut(), ord)
    }

    pub unsafe fn store_shared(&self, val: Shared<T>, ord: Ordering) {
        self.ptr.store(val.deref() as *const _ as *mut _, ord)
    }

    pub fn cas<'a>(&self, old: Shared<T>, new: Owned<T>, ord: Ordering, _: &'a Guard)
                   -> Result<Shared<'a, T>, Owned<T>>
    {
        let old_p = old.deref() as *const _ as *mut _;
        let new_p = new.deref() as *const _ as *mut _;
        if self.ptr.compare_and_swap(old_p, new_p, ord) == old_p {
            unsafe { Ok(mem::transmute(new.deref())) }
        } else {
            Err(new)
        }
    }

    pub fn cas_to_null(&self, old: Shared<T>, ord: Ordering) -> bool {
        let old_p = old.deref() as *const _ as *mut _;
        self.ptr.compare_and_swap(old_p, ptr::null_mut(), ord) == old_p
    }

    pub unsafe fn cas_shared(&self, old: Shared<T>, new: Shared<T>, ord: Ordering) -> bool {
        let old_p = old.deref() as *const _ as *mut _;
        let new_p = new.deref() as *const _ as *mut _;
        self.ptr.compare_and_swap(old_p, new_p, ord) == old_p
    }
}

pub fn pin() -> Guard {
    Guard {
        _dummy: ()
    }
}

pub struct Guard {
    _dummy: ()
}

impl Drop for Guard {
    fn drop(&mut self) {

    }
}
