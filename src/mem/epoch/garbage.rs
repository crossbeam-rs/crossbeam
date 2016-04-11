// Data structures for storing garbage to be freed later (once the
// epochs have sufficiently advanced).
//
// In general, we try to manage the garbage thread locally whenever
// possible. Each thread keep track of three bags of garbage. But if a
// thread is exiting, these bags must be moved into the global garbage
// bags.

use std::ptr;
use std::mem;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, Release, Acquire};

use mem::ZerosValid;

/// One item of garbage.
///
/// Stores enough information to do a deallocation.
#[derive(Debug)]
struct Item {
    ptr: *mut u8,
    free: unsafe fn(*mut u8),
}

/// A single, thread-local bag of garbage.
#[derive(Debug)]
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
                free: free::<T>,
            })
        }
        unsafe fn free<T>(t: *mut u8) {
            drop(Vec::from_raw_parts(t as *mut T, 0, 1));
        }
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    /// Deallocate all garbage in the bag
    pub unsafe fn collect(&mut self) {
        let mut data = mem::replace(&mut self.0, Vec::new());
        for item in data.iter() {
            (item.free)(item.ptr);
        }
        data.truncate(0);
        self.0 = data;
    }
}

// needed because the bags store raw pointers.
unsafe impl Send for Bag {}
unsafe impl Sync for Bag {}

/// A thread-local set of garbage bags.
#[derive(Debug)]
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

    pub fn insert<T>(&mut self, elem: *mut T) {
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
        self.old.len() + self.cur.len() + self.new.len()
    }
}

/// A concurrent garbage bag, currently based on Treiber's stack.
///
/// The elements are themselves owned `Bag`s.
#[derive(Debug)]
pub struct ConcBag {
    head: AtomicPtr<Node>,
}

unsafe impl ZerosValid for ConcBag {}

#[derive(Debug)]
struct Node {
    data: Bag,
    next: AtomicPtr<Node>,
}

impl ConcBag {
    pub fn insert(&self, t: Bag){
        let n = Box::into_raw(Box::new(
            Node { data: t, next: AtomicPtr::new(ptr::null_mut()) }));
        loop {
            let head = self.head.load(Acquire);
            unsafe { (*n).next.store(head, Relaxed) };
            if self.head.compare_and_swap(head, n, Release) == head { break }
        }
    }

    pub unsafe fn collect(&self) {
        // check to avoid xchg instruction
        // when no garbage exists
        let mut head = self.head.load(Relaxed);
        if head != ptr::null_mut() {
            head = self.head.swap(ptr::null_mut(), Acquire);

            while head != ptr::null_mut() {
                let mut n = Box::from_raw(head);
                n.data.collect();
                head = n.next.load(Relaxed);
            }
        }
    }
}
