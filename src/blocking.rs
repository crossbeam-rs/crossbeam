use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::Instant;

pub struct Token<'a, T: 'a> {
    pub parent: &'a Blocking<T>,
    pub _marker: PhantomData<*mut ()>,
}

impl<'a, T> Token<'a, T> {
    pub fn cancel(self) {
        if !self.parent.unregister() {
            self.parent.wake_one();
        }
        mem::forget(self);
    }
}

impl<'a, T> Drop for Token<'a, T> {
    fn drop(&mut self) {
        self.parent.unregister();
    }
}

pub struct Payload<T> {
    pub data: Option<T>,
    pub available: *const AtomicBool,
}

pub struct Blocked<T> {
    pub thread: Thread,
    pub payload: *mut Payload<T>,
}

pub struct Blocking<T> {
    queue: Mutex<VecDeque<Blocked<T>>>,
    // len: AtomicUsize,
}

impl<T> Blocking<T> {
    pub fn new() -> Self {
        Blocking {
            queue: Mutex::new(VecDeque::new()),
            // len: AtomicUsize::new(0),
        }
    }

    pub fn register(&self) -> Token<T> {
        self.register_with(ptr::null_mut())
    }

    pub fn register_with(&self, payload: *mut Payload<T>) -> Token<T> {
        let mut queue = self.queue.lock().unwrap();
        queue.push_back(Blocked {
            thread: thread::current(),
            payload: payload,
        });
        // self.len.store(queue.len(), SeqCst);

        Token {
            parent: self,
            _marker: PhantomData,
        }
    }

    fn unregister(&self) -> bool {
        let mut queue = self.queue.lock().unwrap();
        let current_id = thread::current().id();

        for i in 0..queue.len() {
            if queue[i].thread.id() == current_id {
                queue.remove(i);
                // self.len.store(queue.len(), SeqCst);
                return true;
            }
        }
        false
    }

    // pub fn len(&self) -> usize {
    //     self.len.load(SeqCst)
    // }
    //
    // pub fn is_empty(&self) -> bool {
    //     self.len() == 0
    // }

    pub fn wake_one(&self) {
        // if self.len.load(SeqCst) > 0 {
            let mut queue = self.queue.lock().unwrap();
            if let Some(f) = queue.pop_front() {
                // self.len.store(queue.len(), SeqCst);
                f.thread.unpark();
            }
        // }
    }

    pub fn wake_all(&self) {
        // if self.len.load(SeqCst) > 0 {
            let mut queue = self.queue.lock().unwrap();
            while let Some(f) = queue.pop_front() {
                // self.len.store(queue.len(), SeqCst);
                f.thread.unpark();
            }
        // }
    }

    pub fn send_one(&self, value: T) -> Result<(), T> {
        let mut queue = self.queue.lock().unwrap();
        if let Some(mut f) = queue.pop_front() {
            // self.len.store(queue.len(), SeqCst);

            // if unsafe { !(*f.payload.as_ref().unwrap().available).swap(true, Relaxed) } {
            if true {
                unsafe {
                    f.payload.as_mut().unwrap().data = Some(value);
                }
                f.thread.unpark();
                return Ok(());
            }

            f.thread.unpark();
        }
        Err(value)
    }

    pub fn recv_one(&self) -> Result<T, ()> {
        let mut queue = self.queue.lock().unwrap();
        if let Some(mut f) = queue.pop_front() {
            // self.len.store(queue.len(), SeqCst);

            // if unsafe { !(*f.payload.as_ref().unwrap().available).swap(true, Relaxed) } {
            if true {
                let value = unsafe {
                    f.payload.as_mut().unwrap().data.take().unwrap()
                };
                f.thread.unpark();
                return Ok(value);
            }

            f.thread.unpark();
        }
        Err(())
    }
}
