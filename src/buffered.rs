use std::cell::Cell;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use super::SendError;
use super::TrySendError;
use super::RecvError;
use super::TryRecvError;

struct Node<T> {
    lap: AtomicUsize,
    value: T,
}

pub struct Queue<T> {
    buffer: *mut UnsafeCell<Node<T>>, // !Send + !Sync
    cap: usize,
    power: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    closed: AtomicBool,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0);
        assert!(cap <= ::std::isize::MAX as usize);

        let power = cap.next_power_of_two();

        let mut v = Vec::with_capacity(cap);
        let buffer = v.as_mut_ptr();
        mem::forget(v);
        unsafe { ptr::write_bytes(buffer, 0, cap) }

        Queue {
            buffer: buffer,
            cap: cap,
            power: power,
            head: AtomicUsize::new(power),
            tail: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        let cap = self.cap;
        let power = self.power;
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

                if self.tail.compare_and_swap(tail, new, SeqCst) == tail {
                    unsafe {
                        (*cell).value = value;
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Ok(());
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return Err(TrySendError::Full(value));
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let cap = self.cap;
        let power = self.power;
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

                if self.head.compare_and_swap(head, new, SeqCst) == head {
                    unsafe {
                        let value = ptr::read(&(*cell).value);
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Ok(value);
                    }
                }
            } else if self.closed.load(SeqCst) {
                return Err(TryRecvError::Disconnected);
            } else if clap.wrapping_add(power) == lap {
                return Err(TryRecvError::Empty);
            }
        }
    }

    pub fn close(&self) -> bool {
        self.closed.swap(true, SeqCst) == false
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }
}

// TODO: impl Drop

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::thread;

    #[test]
    fn simple() {
        const STEPS: usize = 1_000_000;

        let q = Arc::new(Queue::with_capacity(5));

        let t = {
            let q = q.clone();
            thread::spawn(move || {
                for i in 0..STEPS {
                    q.send(5);
                }
                println!("SEND DONE");
            })
        };

        for _ in 0..STEPS {
            q.recv();
        }
        println!("RECV DONE");

        t.join().unwrap();
    }
}
