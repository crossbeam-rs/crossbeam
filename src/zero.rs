use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use super::SendError;
use super::TrySendError;
use super::SendTimeoutError;
use super::RecvError;
use super::TryRecvError;
use super::RecvTimeoutError;

use blocking::Blocking;

// TODO: data and thread will get destructed in Drop, which is wrong if version is not FULL
pub struct Queue<T> {
    data: UnsafeCell<T>,
    thread: UnsafeCell<Thread>,
    version: AtomicUsize,

    closed: AtomicBool,

    senders: Blocking,
    receivers: Blocking,
    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

// TODO: sender must wait until its data is taken?

enum State {
    Empty,
    Writing,
    Full,
    Reading,
}

impl State {
    fn from_version(version: usize) -> Self {
        match version % 4 {
            0 => State::Empty,
            1 => State::Writing,
            2 => State::Full,
            _ => State::Reading,
        }
    }
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            data: unsafe { UnsafeCell::new(mem::uninitialized()) },
            thread: unsafe { UnsafeCell::new(mem::uninitialized()) },
            version: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            senders: Blocking::new(),
            receivers: Blocking::new(),
            _marker: PhantomData,
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            return Err(TrySendError::Disconnected(value));
        }

        loop {
            let version = self.version.load(Relaxed);

            match State::from_version(version) {
                State::Empty => {
                    if self.version.compare_and_swap(version, version.wrapping_add(1), SeqCst) == version {
                        unsafe {
                            *self.data.get() = value;
                            *self.thread.get() = thread::current();
                        }
                        self.version.store(version.wrapping_add(2), Release);

                        self.receivers.wake_one();
                        return Ok(());
                    }
                }
                State::Reading => {},
                State::Writing | State::Full => return Err(TrySendError::Full(value)),
            }
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        let mut blocking = false;
        loop {
            let token;
            if blocking {
                token = self.senders.register();
            }

            match self.try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(v)) => {
                    return Err(SendTimeoutError::Disconnected(v));
                }
                Err(TrySendError::Full(v)) => {
                    value = v;

                    if blocking {
                        if let Some(end) = deadline {
                            let now = Instant::now();
                            if now >= end {
                                return Err(SendTimeoutError::Timeout(value));
                            } else {
                                thread::park_timeout(end - now);
                            }
                        } else {
                            thread::park();
                        }
                    }

                    blocking = !blocking;
                }
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        loop {
            let version = self.version.load(Relaxed);

            match State::from_version(version) {
                State::Full => {
                    if self.version.compare_and_swap(version, version.wrapping_add(1), SeqCst) == version {
                        unsafe {
                            let value = ptr::read(self.data.get());
                            let thread = ptr::read(self.thread.get());

                            self.version.store(version.wrapping_add(2), Release);
                            self.senders.wake_one();
                            return Ok(value);
                        }
                    }
                }
                State::Writing => {},
                State::Reading | State::Empty => {
                    if self.closed.load(SeqCst) {
                        return Err(TryRecvError::Disconnected);
                    } else {
                        return Err(TryRecvError::Empty);
                    }
                }
            }
        }
    }

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        let mut blocking = false;
        loop {
            let token;
            if blocking {
                token = self.receivers.register();
            }

            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Disconnected) => {
                    return Err(RecvTimeoutError::Disconnected);
                }
                Err(TryRecvError::Empty) => {
                    if blocking {
                        if let Some(end) = deadline {
                            let now = Instant::now();
                            if now >= end {
                                return Err(RecvTimeoutError::Timeout);
                            } else {
                                thread::park_timeout(end - now);
                            }
                        } else {
                            thread::park();
                        }
                    }
                    blocking = !blocking;
                }
            }
        }
    }

    pub fn close(&self) -> bool {
        if !self.closed.swap(true, SeqCst) {
            self.senders.wake_all();
            self.receivers.wake_all();
            true
        } else {
            false
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }
}

// TODO: impl Drop
