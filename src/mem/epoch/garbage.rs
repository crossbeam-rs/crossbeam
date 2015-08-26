use alloc::heap;

use std::ptr;
use std::mem;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, Release};

struct Item {
    ptr: *mut u8,
    size: usize,
    align: usize,
}

pub struct Bag(Vec<Item>);

impl Bag {
    fn new() -> Bag {
        Bag(vec![])
    }

    fn insert<T>(&mut self, elem: *mut T) {
        let size = mem::size_of::<T>();
        if size > 0 {
            self.0.push(Item {
                ptr: elem as *mut u8,
                size: size,
                align: mem::align_of::<T>(),
            })
        }
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    pub unsafe fn collect(&mut self) {
        for item in self.0.drain(..) {
            heap::deallocate(item.ptr, item.size, item.align);
        }
    }
}

unsafe impl Send for Bag {}

// FIXME: switch this to use modular arithmetic and accessors instead
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
        self.new.insert(elem)
    }

    pub unsafe fn collect(&mut self) {
        let ret = self.old.collect();
        mem::swap(&mut self.old, &mut self.cur);
        mem::swap(&mut self.cur, &mut self.new);
        ret
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
