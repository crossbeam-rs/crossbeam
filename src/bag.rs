use std::ptr;
use std::mem;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, Acquire, Release};

pub struct Bag<T> {
    head: AtomicPtr<Node<T>>,
}

struct Node<T> {
    data: T,
    next: AtomicPtr<Node<T>>,
}

impl<T: Send> Bag<T> {
    pub const fn new() -> Bag<T> {
        Bag { head: AtomicPtr::new(0 as *mut _) }
    }

    pub fn insert(&self, t: T) -> *const T {
        let n = Box::into_raw(Box::new(
            Node { data: t, next: AtomicPtr::new(ptr::null_mut()) })) as *mut Node<T>;
        loop {
            let head = self.head.load(Relaxed);
            unsafe { (*n).next.store(head, Relaxed) };
            if self.head.compare_and_swap(head, n, Release) == head { break }
        }
        unsafe { &(*n).data }
    }

    pub unsafe fn iter_clobber(&self) -> IterClobber<T> {
        let out = self.head.load(Relaxed);
        self.head.store(ptr::null_mut(), Relaxed);
        mem::transmute(out)
    }
}

struct IterClobberNode<T> {
    data: T,
    next: Option<Box<IterClobberNode<T>>>,
}

pub struct IterClobber<T>(Option<Box<IterClobberNode<T>>>);

impl<T> Iterator for IterClobber<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        let cur = self.0.take();
        cur.map(|box IterClobberNode { data, next }| {
            self.0 = next;
            data
        })
    }
}
