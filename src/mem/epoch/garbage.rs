use std::ptr;
use std::mem;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, Release};

trait AnyType {}
impl<T: ?Sized> AnyType for T {}

pub struct Bag(Vec<*mut AnyType>);

impl Bag {
    fn new() -> Bag {
        Bag(vec![])
    }

    fn insert(&mut self, elem: *mut AnyType) {
        self.0.push(elem)
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    pub unsafe fn collect(&mut self) {
        for g in self.0.drain(..) {
            mem::drop(Box::from_raw(g))
        }
    }
}

unsafe impl Send for Bag {}

pub struct Local {
    pub old: Bag,
    pub cur: Bag,
}

impl Local {
    pub fn new() -> Local {
        Local {
            old: Bag::new(),
            cur: Bag::new(),
        }
    }

    pub unsafe fn reclaim<T>(&mut self, elem: *mut T) {
        let elem: *mut AnyType = elem;
        self.cur.insert(
            // forget any borrows within `data`:
            mem::transmute(elem)
        );
    }

    pub unsafe fn collect_one_epoch(&mut self) {
        self.old.collect();
        mem::swap(&mut self.old, &mut self.cur)
    }

    pub unsafe fn collect_all(&mut self) {
        self.old.collect();
        self.cur.collect();
    }

    pub fn size(&self) -> usize {
        self.old.len() + self.cur.len()
    }
}

pub struct ConcBag {
    head: AtomicPtr<Node>,
}

struct Node {
    data: Bag,
    next: AtomicPtr<Node>,
}

impl ConcBag {
    pub const fn new() -> ConcBag {
        ConcBag { head: AtomicPtr::new(0 as *mut _) }
    }

    pub fn insert(&self, t: Bag){
        let n = Box::into_raw(Box::new(
            Node { data: t, next: AtomicPtr::new(ptr::null_mut()) })) as *mut Node;
        loop {
            let head = self.head.load(Relaxed);
            unsafe { (*n).next.store(head, Relaxed) };
            if self.head.compare_and_swap(head, n, Release) == head { break }
        }
    }

    pub unsafe fn collect(&self) {
        let mut head = self.head.load(Relaxed);
        self.head.store(ptr::null_mut(), Relaxed);

        while head != ptr::null_mut() {
            let mut n = Box::from_raw(head);
            n.data.collect();
            head = n.next.load(Relaxed);
        }
    }
}
