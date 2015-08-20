use std::ptr;
use std::mem;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, Release};
use std::vec;

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

    fn capacity(&self) -> usize {
        self.0.capacity()
    }

    pub unsafe fn collect(&mut self) -> Collect {
        Collect(mem::replace(&mut self.0, vec![]).into_iter())
    }
}

struct Collect(vec::IntoIter<*mut AnyType>);

impl Iterator for Collect {
    type Item = Box<AnyType>;

    fn next(&mut self) -> Option<Box<AnyType>> {
        unsafe { self.0.next().map(|p| Box::from_raw(p)) }
    }
}

unsafe impl Send for Bag {}

pub struct Local {
    pub old: Bag,
    pub cur: Bag,
    pub new: Bag,
}

impl Local {
    pub fn new() -> Local {
        Local {
            old: Bag::new(),
            cur: Bag::new(),
            new: Bag::new(),
        }
    }

    pub unsafe fn reclaim<T>(&mut self, elem: *mut T) {
        let elem: *mut AnyType = elem;
        self.new.insert(
            // forget any borrows within `data`:
            mem::transmute(elem)
        );
    }

    pub unsafe fn collect(&mut self) -> Collect {
        let ret = self.old.collect();
        mem::swap(&mut self.old, &mut self.cur);
        mem::swap(&mut self.cur, &mut self.new);
        ret
    }

    pub fn size(&self) -> usize {
        self.old.len() + self.cur.len()
    }

    pub fn capacity(&self) -> usize {
        self.old.capacity() + self.cur.capacity()
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
