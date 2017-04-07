use std::cell::UnsafeCell;
use std::fmt;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

struct Node<T> {
    lap: AtomicUsize,
    value: T,
}

struct BoundedQueue<T> {
    buffer: *mut UnsafeCell<Node<T>>, // !Send + !Sync
    cap: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    closed: AtomicBool,
}

// enum Waiters<T> {
//     receivers: WaitQueue<T>
// }

// struct Blocked<T> {
//     _marker: std::marker::PhantomData<T>,
// }

// struct Waiters<T> {
//     head: AtomicPtr<Blocked<T>>,
//     tail: AtomicPtr<Blocked<T>>,
// }

unsafe impl<T: Send> Send for BoundedQueue<T> {}
unsafe impl<T: Send> Sync for BoundedQueue<T> {}

impl<T> BoundedQueue<T> {
    pub fn with_capacity(cap: usize) -> Self {
        let power = cap.next_power_of_two();

        let mut v = Vec::with_capacity(cap);
        let buffer = v.as_mut_ptr();
        mem::forget(v);
        unsafe { ptr::write_bytes(buffer, 0, cap) }

        BoundedQueue {
            buffer: buffer,
            cap: cap,
            head: AtomicUsize::new(power),
            tail: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    pub fn try_send(&self, value: T) {
    }

    // pub fn send(&self, mut value: T) {
    //     loop {
    //         match self.push(value) {
    //             Ok(()) => receivers.notify();
    //             Err(v) => value = v,
    //         }
    //
    //         let waiters = self.waiters.lock().unwrap();
    //
    //     }
    // }

    pub fn push(&self, value: T) -> Result<(), T> {
        let cap = self.cap;
        let power = cap.next_power_of_two();
        let buffer = self.buffer;

        loop {
            let tail = self.tail.load(Relaxed);
            let pos = tail & (power - 1);
            let lap = tail & !(power - 1);

            let cell = unsafe { (*buffer.offset(pos as isize)).get() };
            let clap = unsafe { (*cell).lap.load(Acquire) };

            if lap == clap {
                let new = if pos + 1 < cap {
                    tail + 1
                } else {
                    lap.wrapping_add(power).wrapping_add(power)
                };

                if self.tail.compare_and_swap(tail, new, Relaxed) == tail {
                    unsafe {
                        (*cell).value = value;
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Ok(());
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return Err(value);
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        let cap = self.cap;
        let power = cap.next_power_of_two();
        let buffer = self.buffer;

        loop {
            let head = self.head.load(Relaxed);
            let pos = head & (power - 1);
            let lap = head & !(power - 1);

            let cell = unsafe { (*buffer.offset(pos as isize)).get() };
            let clap = unsafe { (*cell).lap.load(Acquire) };

            if lap == clap {
                let new = if pos + 1 < cap {
                    head + 1
                } else {
                    lap.wrapping_add(power).wrapping_add(power)
                };

                if self.head.compare_and_swap(head, new, Relaxed) == head {
                    unsafe {
                        let value = ptr::read(&(*cell).value);
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Some(value);
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return None;
            }
        }
    }
}

enum Inner<T> {
    Bounded(Arc<BoundedQueue<T>>),
    // Unbounded(Arc<UnboundedQueue<T>>),
}

struct Sender<T>(Inner<T>);

struct Receiver<T>(Inner<T>);

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::thread;

    #[test]
    fn simple() {
        let q = Arc::new(BoundedQueue::with_capacity(5));

        let t = {
            let q = q.clone();
            thread::spawn(move || {
                for i in 0..10_000_000 {
                    q.push(i);
                }
            })
        };

        for _ in 0..10_000_000 {
            q.pop();
        }

        t.join().unwrap();
    }
}
