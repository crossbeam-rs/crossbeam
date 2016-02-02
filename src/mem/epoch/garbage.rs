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

/// A concurrent garbage bag, currently based on Treiber's stack.
pub struct ConcBag {
    head: AtomicPtr<Node>,
}

unsafe impl ZerosValid for ConcBag {}

struct Node {
    ptr: *mut u8,
    free: unsafe fn(*mut u8),
    next: AtomicPtr<Node>,
}

impl ConcBag {
    pub fn insert<T>(&self, elem: *mut T) {
        unsafe fn free<T>(t: *mut u8) {
            drop(Vec::from_raw_parts(t as *mut T, 0, 1));
        }

        if mem::size_of::<T>() == 0 { return; }

        let n = Box::into_raw(Box::new(Node {
            ptr: elem as *mut u8,
            free: free::<T>,
            next: AtomicPtr::new(ptr::null_mut())
        }));

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
                let n = Box::from_raw(head);
                (n.free)(n.ptr);
                head = n.next.load(Relaxed);
            }
        }
    }
}
