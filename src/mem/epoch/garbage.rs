//! Data structures for storing garbage to be freed later.
//!
//! In general, we try to manage the garbage thread locally whenever possible. Each thread keep
//! track of three bags of garbage. But if a thread is exiting, these bags must be moved into the
//! global garbage bags.

use std::{mem, ptr};
use std::sync::atomic::{self, AtomicPtr};

use mem::ZerosValid;

/// One item of garbage.
///
/// This stores information for deallocating garbage.
#[derive(Debug)]
struct Item {
    /// Pointer to the first byte in the buffer.
    ptr: *mut u8,
    /// The destructor function pointer.
    ///
    /// This takes `self.ptr` as argument and deallocates it.
    dtor: unsafe fn(*mut u8),
}

/// A single, thread-local bag of garbage.
#[derive(Debug)]
pub struct Bag(Vec<Item>);

impl Bag {
    /// Create a new, empty bag of garbage.
    fn new() -> Bag {
        Bag(Vec::new())
    }

    /// Insert a pointer to be deallocated when the garbage is collected.
    ///
    /// This inserts `elem` into the bag of garbage with the destructor deallocating it.
    fn insert<T>(&mut self, elem: *mut T) {
        /// The freeing destructor.
        unsafe fn dtor<T>(t: *mut u8, len: usize) {
            // Construct a vector and drop it.
            drop(Vec::from_raw_parts(t as *mut T, 0, 1));
        }

        // Obtain the size of the element.
        let size = mem::size_of::<T>();
        // We need not drop empty buffers.
        if size > 0 {
            // Simply push it to the vector of garbage.
            self.0.push(Item {
                ptr: elem as *mut u8,
                dtor: dtor::<T>,
            })
        }
    }

    /// Get the number of items in this garbage bag.
    fn len(&self) -> usize {
        self.0.len()
    }

    /// Run all destructors of the garbage in the bag.
    pub unsafe fn collect(&mut self) {
        // Go over all the garbage, remove it and run the destructors.
        for item in data.drain(..) {
            (item.dtor)(item.ptr);
        }
    }
}

// Needed because the bags store raw pointers.
unsafe impl Send for Bag {}
unsafe impl Sync for Bag {}

/// A thread-local set of garbage bags.
#[derive(Debug)]
pub struct Local {
    /// Garbage added at least one epoch behind the current local epoch.
    pub old: Bag,
    /// Garbage added in the current local epoch or earlier.
    pub cur: Bag,
    /// Garbage added in the current _global_ epoch.
    pub new: Bag,
}

impl Local {
    /// Create an empty garbage set.
    pub fn new() -> Local {
        Local {
            old: Bag::new(),
            cur: Bag::new(),
            new: Bag::new(),
        }
    }

    /// Insert a new element to be deallocated eventually.
    pub fn insert<T>(&mut self, elem: *mut T) {
        self.new.insert(elem)
    }

    /// Collect one epoch of garbage and rotate the local garbage bags.
    pub unsafe fn collect(&mut self) {
        // All the old garbage will be destroyed.
        let ret = self.old.collect();
        // Then rotate the garbage (note that `self.old` is now empty).
        mem::swap(&mut self.old, &mut self.cur);
        mem::swap(&mut self.cur, &mut self.new);

        ret
    }

    /// Get the total number of garbage items in the set.
    pub fn size(&self) -> usize {
        // Sum the bags.
        self.old.len() + self.cur.len() + self.new.len()
    }
}

/// A concurrent garbage bag.
///
/// This is currently based on Treiber's stack. The elements are themselves owned `Bag`s.
#[derive(Debug)]
pub struct ConcBag {
    /// The top node of the stack.
    head: AtomicPtr<Node>,
}

/// A node in the Treiber stack of garbage bags.
#[derive(Debug)]
struct Node {
    /// The content of this node.
    data: Bag,
    /// The node below.
    next: AtomicPtr<Node>,
}


impl ConcBag {
    /// Insert a bag of garbage.
    pub fn insert(&self, t: Bag) {
        // Allocate the new node on the heap (so we can get an unbound pointer).
        let n = Box::into_raw(Box::new(Node {
            data: t,
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        // Spin until the ABA problem is solved.
        // TODO: Use striping to avoid this from spin multiple times (not as unlikely as you can
        //       think, especially if several threads constantly creates new garbage).
        loop {
            // Load the head.
            let head = self.head.load(atomic::Ordering::Acquire);
            // Store the loaded head in the node which might become the new head node.
            unsafe { (*n).next.store(head, atomic::Ordering::Relaxed); }
            // To avoid leaking nodes, we use CAS to ensure that our snapshot (`head`) is still
            // valid. If it is, we can replace the node with node `n` linking it and break the
            // loop. If not, we will have to spin again until the ABA condition is over.
            if self.head.compare_and_swap(head, n, atomic::Ordering::Release) == head {
                break;
            }
        }
    }

    /// Run the destructor of all the garbage.
    pub unsafe fn collect(&self) {
        // Fetch the head node.
        let mut head = self.head.load(atomic::Ordering::Relaxed);
        // Make sure it isn't null (if it is, the stack is empty).
        if head != ptr::null_mut() {
            // Take the head and leave a null pointer in its place (clear the stack).
            head = self.head.swap(ptr::null_mut(), atomic::Ordering::Acquire);

            // Now that we have obtained exclusive access to the stack, we can simply iterate over
            // the nodes and collect one by one.
            while head != ptr::null_mut() {
                // Reconstruct the node's pointer.
                let mut n = Box::from_raw(head);
                // Then run destructors of the bag.
                n.data.collect();
                // The go to next node.
                head = n.next.load(atomic::Ordering::Relaxed);
                // Repeat until no more nodes.
            }
        }
    }
}

// Null pointer is a valid and null `Vec<T>` is also.
unsafe impl ZerosValid for ConcBag {}
