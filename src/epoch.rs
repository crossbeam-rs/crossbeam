use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::atomic::Ordering::{self, Relaxed, Acquire, Release, SeqCst};
use std::ops::{Deref, DerefMut};

use bag::Bag;
use cache_padded::CachePadded;

trait AnyType {}
impl<T: ?Sized> AnyType for T {}

struct Participants {
    bag: Bag<CachePadded<Participant>>,
}

struct Participant {
    epoch: AtomicUsize,
    active: AtomicUsize,
    op_count: u32,
}

impl Participants {
    const fn new() -> Participants {
        Participants { bag: Bag::new() }
    }

    fn enroll(&self) -> *mut Participant {
        let participant = Participant {
            epoch: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            op_count: 0,
        };
        unsafe {
            self.bag.insert(CachePadded::new(participant)) as *mut _
        }
    }
}

struct EpochState {
    epoch: CachePadded<AtomicUsize>,
    garbage: [CachePadded<Bag<*mut AnyType>>; 3],
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

impl Participant {
    fn enter(&mut self) {
        self.active.store(self.active.load(Relaxed) + 1, Relaxed);
        atomic::fence(SeqCst);

        let epoch = EPOCH.epoch.load(Relaxed);
        if epoch == self.epoch.load(Relaxed) {
            self.op_count = self.op_count.saturating_add(1);
        } else {
            self.epoch.store(epoch, Relaxed);
            self.op_count = 0;
        }
    }

    fn exit(&mut self) {
        self.active.store(self.active.load(Relaxed) - 1, Release);
    }

    fn reclaim<T>(&mut self, data: *mut T) {
        let data: *mut AnyType = data;
        EPOCH.garbage[self.epoch.load(Relaxed)]
             .insert(unsafe {
                 // forget any borrows within `data`:
                 mem::transmute(data)
             });
    }
}


pub fn try_collect() -> bool {
    let cur_epoch = EPOCH.epoch.load(SeqCst);

    for p in EPOCH.participants.bag.iter() {
        if p.active.load(Relaxed) > 0 && p.epoch.load(Relaxed) != cur_epoch {
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

impl<'a, T> Shared<'a, T> {
    unsafe fn from_raw(raw: *mut T) -> Shared<'a, T> {
        Shared { data: mem::transmute(raw) }
    }

    unsafe fn from_ref(r: &T) -> Shared<'a, T> {
        Shared { data: mem::transmute(r) }
    }

    unsafe fn from_owned(owned: Owned<T>) -> Shared<'a, T> {
        Shared::from_ref(owned.deref())
    }

    fn as_raw(&self) -> *mut T {
        self.data as *const _ as *mut _
    }
}

impl<T> Owned<T> {
    fn as_raw(&self) -> *mut T {
        self.deref() as *const _ as *mut _
    }
}

pub struct AtomicPtr<T> {
    ptr: atomic::AtomicPtr<T>,
}

impl<T> Default for AtomicPtr<T> {
    fn default() -> AtomicPtr<T> {
        AtomicPtr { ptr: atomic::AtomicPtr::new(ptr::null_mut()) }
    }
}

impl<T> AtomicPtr<T> {
    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> Option<Shared<'a, T>> {
        let p = self.ptr.load(ord);
        if p == ptr::null_mut() {
            None
        } else {
            Some(unsafe { Shared::from_raw(p) })
        }
    }

    pub fn store<'a>(&self, val: Owned<T>, ord: Ordering, _: &'a Guard) -> Shared<'a, T> {
        unsafe {
            let shared = Shared::from_owned(val);
            self.store_shared(shared, ord);
            shared
        }
    }

    pub fn store_null(&self, ord: Ordering) {
        self.ptr.store(ptr::null_mut(), ord)
    }

    pub unsafe fn store_shared(&self, val: Shared<T>, ord: Ordering) {
        self.ptr.store(val.as_raw(), ord)
    }

    pub fn cas<'a>(&self, old: Shared<T>, new: Owned<T>, ord: Ordering, _: &'a Guard)
                   -> Result<Shared<'a, T>, Owned<T>>
    {
        if self.ptr.compare_and_swap(old.as_raw(), new.as_raw(), ord) == old.as_raw() {
            Ok(unsafe { Shared::from_owned(new) })
        } else {
            Err(new)
        }
    }

    pub fn cas_to_null(&self, old: Shared<T>, ord: Ordering) -> bool {
        self.ptr.compare_and_swap(old.as_raw(), ptr::null_mut(), ord) == old.as_raw()
    }

    pub unsafe fn cas_shared(&self, old: Shared<T>, new: Shared<T>, ord: Ordering) -> bool {
        self.ptr.compare_and_swap(old.as_raw(), new.as_raw(), ord) == old.as_raw()
    }

    pub fn swap<'a>(&self, new: Owned<T>, ord: Ordering, _: &'a Guard) -> Shared<'a, T> {
        unsafe { Shared::from_raw(self.ptr.swap(new.as_raw(), ord)) }
    }

    pub fn swap_null<'a>(&self, ord: Ordering, _: &'a Guard) -> Shared<'a, T> {
        unsafe { Shared::from_raw(self.ptr.swap(ptr::null_mut(), ord)) }
    }
}

thread_local!(static LOCAL_EPOCH: *mut Participant = EPOCH.participants.enroll());

pub fn pin() -> Guard {
    LOCAL_EPOCH.with(|p| unsafe { (**p).enter() });
    Guard {
        _dummy: ()
    }
}

pub struct Guard {
    _dummy: ()
}

impl Guard {
    pub fn unlinked<T>(&self, val: Shared<T>) {
        LOCAL_EPOCH.with(|p| unsafe { (**p).reclaim(val.as_raw()) })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        LOCAL_EPOCH.with(|p| unsafe { (**p).exit() });
    }
}
