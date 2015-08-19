use std::cell::RefCell;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{self, Relaxed, Acquire, Release, SeqCst};
use std::ops::{Deref, DerefMut};

use std::io::stderr;
use std::io::prelude::*;

use cache_padded::CachePadded;

mod garbage;

fn log(s: &str) {
    let _ = stderr().write(s.as_bytes());
    let _ = stderr().write("\n".as_bytes());
}

struct Participants {
    head: AtomicPtr<ParticipantNode>
}

//type ParticipantNode = CachePadded<Participant>;
struct ParticipantNode(Participant);

impl ParticipantNode {
    fn new(p: Participant) -> ParticipantNode {
        ParticipantNode(p)
    }
}

impl Deref for ParticipantNode {
    type Target = Participant;
    fn deref(&self) -> &Participant {
        &self.0
    }
}

impl DerefMut for ParticipantNode {
    fn deref_mut(&mut self) -> &mut Participant {
        &mut self.0
    }
}

struct Participant {
    epoch: AtomicUsize,
    in_critical: AtomicUsize,
    active: AtomicBool,
    garbage: RefCell<garbage::Local>,
    next: AtomicPtr<ParticipantNode>,
}

impl Participants {
    const fn new() -> Participants {
        Participants { head: AtomicPtr::new() }
    }

    fn enroll(&self) -> *const Participant {
        let mut participant = Owned::new(ParticipantNode::new(
            Participant {
                epoch: AtomicUsize::new(0),
                in_critical: AtomicUsize::new(0),
                active: AtomicBool::new(true),
                garbage: RefCell::new(garbage::Local::new()),
                next: AtomicPtr::new(),
            }
        ));
        let fake_guard = ();
        let g: &'static Guard = unsafe { mem::transmute(&fake_guard) };
        loop {
            let head = self.head.load(Relaxed, g);
            unsafe { participant.next.store_shared(head, Relaxed) };
            match self.head.cas_and_ref(head, participant, Release, g) {
                Ok(shared) => {
                    let shared: &Participant = &shared;
                    return shared;
                }
                Err(owned) => {
                    participant = owned;
                }
            }
        }
    }

    fn iter<'a>(&'a self, g: &'a Guard) -> Iter<'a> {
        Iter {
            guard: g,
            next: &self.head,
            needs_acq: true,
        }
    }
}

struct Iter<'a> {
    guard: &'a Guard,
    next: &'a AtomicPtr<ParticipantNode>,
    needs_acq: bool,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Participant;
    fn next(&mut self) -> Option<&'a Participant> {
        let mut cur = if self.needs_acq {
            self.needs_acq = false;
            self.next.load(Acquire, self.guard)
        } else {
            self.next.load(Relaxed, self.guard)
        };

        while let Some(n) = cur {
            if !n.active.load(Relaxed) {
                cur = n.next.load(Relaxed, self.guard);
                let unlinked = unsafe { self.next.cas_shared(Some(n), cur, Relaxed) };
                if unlinked { unsafe { self.guard.unlinked(n) } }
                self.next = &n.next;
            } else {
                self.next = &n.next;
                return Some(&n)
            }
        }

        None
    }
}

struct EpochState {
    epoch: CachePadded<AtomicUsize>,
    garbage: [CachePadded<garbage::ConcBag>; 3],
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
    fn enter(&self) {
        self.in_critical.store(self.in_critical.load(Relaxed) + 1, Relaxed);
        atomic::fence(SeqCst);

        let epoch = EPOCH.epoch.load(Relaxed);
        let delta = epoch - self.epoch.load(Relaxed);
        if delta > 0 {
            self.epoch.store(epoch, Relaxed);

            unsafe {
                // Note: carefully designed to allow re-entrancy via drop, by
                // performing the drops *after* the borrow_mut() is relinquished

                let gen1 = self.garbage.borrow_mut().collect();
                if delta == 1 {
                    for g in gen1 {
                        mem::drop(g);
                    }
                } else {
                    let gen2 = self.garbage.borrow_mut().collect();

                    for g in gen1 {
                        mem::drop(g);
                    }
                    for g in gen2 {
                        mem::drop(g);
                    }
                }
            }
        }
    }

    fn exit(&self) {
        self.in_critical.store(self.in_critical.load(Relaxed) - 1, Release);
    }

    unsafe fn reclaim<T>(&self, data: *mut T) {
        self.garbage.borrow_mut().reclaim(data);
    }

    fn try_collect(&self) -> bool {
        let cur_epoch = EPOCH.epoch.load(SeqCst);

        let fake_guard = ();
        let g: &'static Guard = unsafe { mem::transmute(&fake_guard) };

        for p in EPOCH.participants.iter(g) {
            if p.in_critical.load(Relaxed) > 0 && p.epoch.load(Relaxed) != cur_epoch {
                return false
            }
        }

        let new_epoch = cur_epoch.wrapping_add(1);
        atomic::fence(Acquire);
        if EPOCH.epoch.compare_and_swap(cur_epoch, new_epoch, SeqCst) != cur_epoch {
            return false
        }

        unsafe {
            EPOCH.garbage[new_epoch.wrapping_add(1) % 3].collect();
            for g in self.garbage.borrow_mut().collect() {
                mem::drop(g);
            }
        }

        true
    }

    fn migrate_garbage(&self) {
        let cur_epoch = self.epoch.load(Relaxed);
        let local = mem::replace(&mut *self.garbage.borrow_mut(), garbage::Local::new());
        EPOCH.garbage[cur_epoch.wrapping_sub(1) % 3].insert(local.old);
        EPOCH.garbage[cur_epoch % 3].insert(local.cur);
    }
}

pub struct Owned<T> {
    data: Box<T>,
}

impl<T> Owned<T> {
    pub fn new(t: T) -> Owned<T> {
        Owned { data: Box::new(t) }
    }

    fn as_raw(&self) -> *mut T {
        self.deref() as *const _ as *mut _
    }
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
    type Target = &'a T;
    fn deref(&self) -> &&'a T {
        &self.data
    }
}

impl<'a, T> Shared<'a, T> {
    unsafe fn from_raw(raw: *mut T) -> Option<Shared<'a, T>> {
        if raw == ptr::null_mut() { None }
        else {
            Some(Shared {
                data: mem::transmute::<*mut T, &T>(raw)
            })
        }
    }

    unsafe fn from_ref(r: &T) -> Shared<'a, T> {
        Shared { data: mem::transmute(r) }
    }

    unsafe fn from_owned(owned: Owned<T>) -> Shared<'a, T> {
        let ret = Shared::from_ref(owned.deref());
        mem::forget(owned);
        ret
    }

    fn as_raw(&self) -> *mut T {
        self.data as *const _ as *mut _
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

fn opt_shared_into_raw<T>(val: Option<Shared<T>>) -> *mut T {
    val.map(|p| p.as_raw()).unwrap_or(ptr::null_mut())
}

fn opt_owned_as_raw<T>(val: &Option<Owned<T>>) -> *mut T {
    val.as_ref().map(Owned::as_raw).unwrap_or(ptr::null_mut())
}

impl<T> AtomicPtr<T> {
    pub const fn new() -> AtomicPtr<T> {
        AtomicPtr { ptr: atomic::AtomicPtr::new(0 as *mut _) }
    }

    pub fn load<'a>(&self, ord: Ordering, _: &'a Guard) -> Option<Shared<'a, T>> {
        unsafe { Shared::from_raw(self.ptr.load(ord)) }
    }

    pub fn store(&self, val: Option<Owned<T>>, ord: Ordering) {
        self.ptr.store(opt_owned_as_raw(&val), ord)
    }

    pub fn store_and_ref<'a>(&self, val: Owned<T>, ord: Ordering, _: &'a Guard) -> Shared<'a, T> {
        unsafe {
            let shared = Shared::from_owned(val);
            self.store_shared(Some(shared), ord);
            shared
        }
    }

    pub unsafe fn store_shared(&self, val: Option<Shared<T>>, ord: Ordering) {
        self.ptr.store(opt_shared_into_raw(val), ord)
    }

    pub fn cas(&self, old: Option<Shared<T>>, new: Option<Owned<T>>, ord: Ordering)
               -> Result<(), Option<Owned<T>>>
    {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old),
                                     opt_owned_as_raw(&new),
                                     ord) == opt_shared_into_raw(old)
        {
            Ok(())
        } else {
            Err(new)
        }
    }

    pub fn cas_and_ref<'a>(&self, old: Option<Shared<T>>, new: Owned<T>,
                           ord: Ordering, _: &'a Guard)
                           -> Result<Shared<'a, T>, Owned<T>>
    {
        if self.ptr.compare_and_swap(opt_shared_into_raw(old), new.as_raw(), ord)
            == opt_shared_into_raw(old)
        {
            Ok(unsafe { Shared::from_owned(new) })
        } else {
            Err(new)
        }
    }

    pub unsafe fn cas_shared(&self, old: Option<Shared<T>>, new: Option<Shared<T>>,
                             ord: Ordering)
                             -> bool
    {
        self.ptr.compare_and_swap(opt_shared_into_raw(old),
                                  opt_shared_into_raw(new),
                                  ord) == opt_shared_into_raw(old)
    }

    pub fn swap<'a>(&self, new: Option<Owned<T>>, ord: Ordering, _: &'a Guard)
                    -> Option<Shared<'a, T>> {
        unsafe { Shared::from_raw(self.ptr.swap(opt_owned_as_raw(&new), ord)) }
    }

    pub fn swap_shared<'a>(&self, new: Option<Shared<T>>, ord: Ordering, _: &'a Guard)
                           -> Option<Shared<'a, T>> {
        unsafe {
            Shared::from_raw(self.ptr.swap(opt_shared_into_raw(new), ord))
        }
    }
}

struct LocalEpoch {
    participant: *const Participant,
}

impl LocalEpoch {
    fn new() -> LocalEpoch {
        LocalEpoch { participant: EPOCH.participants.enroll() }
    }

    fn get(&self) -> &Participant {
        unsafe { &*self.participant }
    }
}

impl Drop for LocalEpoch {
    fn drop(&mut self) {
        let p = self.get();
        p.enter();
        p.try_collect();
        p.migrate_garbage();
        p.exit();
        p.active.store(false, Relaxed);
    }
}

thread_local!(static LOCAL_EPOCH: LocalEpoch = LocalEpoch::new() );

#[must_use]
pub struct Guard {
    _dummy: ()
}

static GC_THRESH: usize = 32;

fn with_participant<F, T>(f: F) -> T where F: FnOnce(&Participant) -> T {
    LOCAL_EPOCH.with(|e| f(e.get()))
}

pub fn garbage_size() -> usize {
    with_participant(|p| p.garbage.borrow().size())
}

pub fn pin() -> Guard {
    let needs_collect = with_participant(|p| {
        p.enter();
        p.garbage.borrow().size() > GC_THRESH
    });
    let g = Guard {
        _dummy: ()
    };

    if needs_collect {
        g.try_collect();
    }

    g
}

impl Guard {
    pub unsafe fn unlinked<T>(&self, val: Shared<T>) {
        with_participant(|p| p.reclaim(val.as_raw()))
    }

    pub fn try_collect(&self) -> bool {
        with_participant(|p| p.try_collect())
    }

    pub fn migrate_garbage(&self) {
        with_participant(|p| p.migrate_garbage())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        with_participant(|p| p.exit());
    }
}

impl !Send for Guard {}
impl !Sync for Guard {}

#[cfg(test)]
mod test {
    use super::{Participants, EPOCH};
    use super::*;

    #[test]
    fn smoke_enroll() {
        Participants::new().enroll();
    }

    #[test]
    fn smoke_enroll_EPOCH() {
        EPOCH.participants.enroll();
    }

    #[test]
    fn smoke_guard() {
        let g = pin();
    }
}
