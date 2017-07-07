use std::collections::VecDeque;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::Instant;

pub struct Token<'a> {
    parent: &'a Blocking,
    _marker: PhantomData<*mut ()>,
}

impl<'a> Token<'a> {
    pub fn cancel(self) {
        if !self.parent.unregister() {
            self.parent.wake_one();
        }
        mem::forget(self);
    }
}

impl<'a> Drop for Token<'a> {
    fn drop(&mut self) {
        self.parent.unregister();
    }
}

pub struct Blocking {
    queue: Mutex<VecDeque<Thread>>,
    len: AtomicUsize,
}

impl Blocking {
    pub fn new() -> Self {
        Blocking {
            queue: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn register(&self) -> Token {
        let mut queue = self.queue.lock().unwrap();
        queue.push_back(thread::current());
        self.len.store(queue.len(), SeqCst);

        Token {
            parent: self,
            _marker: PhantomData,
        }
    }

    fn unregister(&self) -> bool {
        let mut queue = self.queue.lock().unwrap();
        let current_id = thread::current().id();

        for i in 0..queue.len() {
            if queue[i].id() == current_id {
                queue.remove(i);
                self.len.store(queue.len(), SeqCst);
                return true;
            }
        }
        false
    }

    pub fn len(&self) -> usize {
        self.len.load(SeqCst)
    }

    pub fn wake_one(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut queue = self.queue.lock().unwrap();
            if let Some(f) = queue.pop_front() {
                self.len.store(queue.len(), SeqCst);
                f.unpark();
            }
        }
    }

    pub fn wake_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut queue = self.queue.lock().unwrap();
            while let Some(f) = queue.pop_front() {
                self.len.store(queue.len(), SeqCst);
                f.unpark();
            }
        }
    }
}
