use std::cell::UnsafeCell;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};

use super::TrySendError;
use super::TryRecvError;

const EMPTY: usize = 0;
const READING: usize = 1;
const WRITING: usize = 2;
const FULL: usize = 3;

pub struct Queue<T> {
    state: AtomicUsize,
    closed: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

// TODO: sender must wait until its data is taken?

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            state: AtomicUsize::new(EMPTY),
            closed: AtomicBool::new(false),
            data: unsafe { UnsafeCell::new(mem::uninitialized()) },
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        if self.state.compare_and_swap(EMPTY, WRITING, SeqCst) == EMPTY {
            unsafe { *self.data.get() = value; }
            self.state.store(FULL, Release);
            Ok(())
        } else {
            Err(TrySendError::Full(value))
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        if self.state.compare_and_swap(FULL, READING, SeqCst) == FULL {
            let ret = unsafe { ptr::read(self.data.get()) };
            self.state.store(EMPTY, Relaxed);
            Ok(ret)
        } else if self.closed.load(SeqCst) {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
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
