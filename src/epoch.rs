use std::any::Any;
use std::mem;
use std::ptr;
use std::sync::atomic::{self, AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release, SeqCst};
use std::marker;
use std::ops::{Deref, DerefMut};

use self::bag::Bag;
use self::cache_padded::CachePadded;

mod cache_padded {
    use std::marker;
    use std::cell::UnsafeCell;
    use std::mem;
    use std::ptr;
    use std::ops::{Deref, DerefMut};

    // assume a cacheline size of 64 bytes, and that T is smaller than a cacheline
    pub struct CachePadded<T> {
        data: UnsafeCell<[u8; 64]>,
        _marker: marker::PhantomData<T>,
    }

    impl<T> CachePadded<T> {
        pub const fn zeroed() -> CachePadded<T> {
            CachePadded {
                data: UnsafeCell::new([0; 64]),
                _marker: marker::PhantomData,
            }
        }

        // safe to call only when sizeof(T) <= 64
        pub unsafe fn new(t: T) -> CachePadded<T> {
            let ret = CachePadded {
                data: UnsafeCell::new(mem::uninitialized()),
                _marker: marker::PhantomData,
            };
            let p: *mut T = mem::transmute(&ret);
            ptr::write(p, t);
            ret
        }
    }

    impl<T> Deref for CachePadded<T> {
        type Target = T;
        fn deref(&self) -> &T {
            unsafe { mem::transmute(&self.data) }
        }
    }

    impl<T> DerefMut for CachePadded<T> {
        fn deref_mut(&mut self) -> &mut T {
            unsafe { mem::transmute(&mut self.data) }
        }
    }
}

mod bag {
    use std::ptr;
    use std::mem;
    use std::sync::atomic::AtomicPtr;
    use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};

    pub struct Bag<T> {
        pub head: AtomicPtr<Node<T>>
    }

    pub struct Node<T> {
        pub data: T,
        pub next: AtomicPtr<Node<T>>
    }

    impl<T> Bag<T> {
        pub const fn new() -> Bag<T> {
            Bag { head: AtomicPtr::new(0 as *mut _) }
        }

        pub fn insert(&self, t: T) -> *const Node<T> {
            let mut n = Box::into_raw(Box::new(
                Node { data: t, next: AtomicPtr::new(ptr::null_mut()) })) as *mut Node<T>;
            loop {
                let head = self.head.load(Relaxed);
                unsafe { (*n).next.store(head, Relaxed) };
                if self.head.compare_and_swap(head, n, Release) == head { break }
            }
            n as *const _
        }

        pub unsafe fn into_iter_clobber(&self) -> IntoIterClobber<T> {
            let out = self.head.load(Relaxed);
            self.head.store(ptr::null_mut(), Relaxed);
            mem::transmute(out)
        }

        pub fn iter<'a>(&'a self) -> Iter<'a, T> {
            Iter { next: &self.head }
        }
    }

    struct IntoIterNode<T> {
        data: T,
        next: Option<Box<IntoIterNode<T>>>,
    }

    pub struct IntoIterClobber<T>(Option<Box<IntoIterNode<T>>>);

    impl<T> Iterator for IntoIterClobber<T> {
        type Item = T;
        fn next(&mut self) -> Option<T> {
            let cur = self.0.take();
            cur.map(|box IntoIterNode { data, next }| {
                self.0 = next;
                data
            })
        }
    }

    pub struct Iter<'a, T: 'a> {
        next: &'a AtomicPtr<Node<T>>,
    }

    impl<'a, T> Iterator for Iter<'a, T> {
        type Item = &'a T;
        fn next(&mut self) -> Option<&'a T> {
            unsafe {
                let cur = self.next.load(Acquire);
                if cur == ptr::null_mut() { return None }
                let Node { ref data, ref next } = *cur;
                self.next = mem::transmute(next);
                Some(mem::transmute(data))
            }
        }
    }
}

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
            let n = self.bag.insert(CachePadded::new(participant));
            (*n).data.deref()
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
            for g in EPOCH.garbage[(new_epoch + 1) % 3].into_iter_clobber() {
                // the pointer g is now unique; drop it
                mem::drop(Box::from_raw(g))
            }
        }
    }
}
