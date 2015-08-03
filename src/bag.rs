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

impl<T> Bag<T> {
    pub const fn new() -> Bag<T> {
        Bag { head: AtomicPtr::new(0 as *mut _) }
    }

    pub fn insert(&self, t: T) -> *const T {
        let mut n = Box::into_raw(Box::new(
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

    pub fn iter<'a>(&'a self) -> Iter<'a, T> {
        Iter {
            next: &self.head,
            needs_acq: true,
        }
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

pub struct Iter<'a, T: 'a> {
    next: &'a AtomicPtr<Node<T>>,
    needs_acq: bool,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        unsafe {
            let cur;
            if self.needs_acq {
                self.needs_acq = false;
                cur = self.next.load(Acquire);
            } else {
                cur = self.next.load(Relaxed);
            }

            if cur == ptr::null_mut() { return None }
            let Node { ref data, ref next } = *cur;
            self.next = mem::transmute(next);
            Some(mem::transmute(data))
        }
    }
}
