//! Data structure for storing garbage

use alloc::heap;

use std::ptr;
use std::mem;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, Release};

/// One item of garbage.
///
/// Stores enough information to do a deallocation.
struct Item {
    ptr: *mut u8,
    size: usize,
    align: usize,
}

/// A single, thread-local bag of garbage.
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

    /// Deallocate all garbage in the bag
    pub unsafe fn collect(&mut self) {
        for item in self.0.drain(..) {
            heap::deallocate(item.ptr, item.size, item.align);
        }
    }
}

// needed because the bags store raw pointers.
unsafe impl Send for Bag {}

/// A thread-local set of garbage bags.
// FIXME: switch this to use modular arithmetic and accessors instead
pub struct Local {
    /// Garbage added at least one epoch behind the current local epoch
    pub old: Bag,
    /// Garbage added in the current local epoch or earlier
    pub cur: Bag,
    /// Garbage added in the current *global* epoch
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

    pub fn reclaim<T>(&mut self, elem: *mut T) {
        self.new.insert(elem)
    }

    /// Collect one epoch of garbage, rotating the local garbage bags.
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

/// A concurrent garbage bag, currently based on Treiber's stack.
///
/// The elements are themselves owned `Bag`s.
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
