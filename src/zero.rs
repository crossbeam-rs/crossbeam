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
use blocking::Payload;

pub struct Queue<T> {
    lock: Mutex<()>,
    closed: AtomicBool,

    senders: Blocking<T>,
    receivers: Blocking<T>,

    _marker: PhantomData<T>,
}

unsafe impl<T: Send> Send for Queue<T> {}
unsafe impl<T: Send> Sync for Queue<T> {}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            lock: Mutex::new(()),
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

        match self.receivers.send_one(value) {
            Ok(()) => return Ok(()),
            Err(value) => return Err(TrySendError::Full(value)),
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            // TODO: try_send and registration must be enclosed within the same lock guard scope

            match self.try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(v)) => {
                    return Err(SendTimeoutError::Disconnected(v));
                }
                Err(TrySendError::Full(v)) => {
                    // TODO: use unsafecell?
                    let mut payload = Payload {
                        data: Some(v),
                        available: ptr::null_mut(),
                    };
                    {
                        let token = self.senders.register_with(&mut payload);

                        if let Some(end) = deadline {
                            let now = Instant::now();

                            if now >= end {
                                drop(token);

                                match payload.data {
                                    None => return Ok(()),
                                    Some(v) => return Err(SendTimeoutError::Timeout(v)),
                                }
                            } else {
                                thread::park_timeout(end - now);
                            }
                        } else {
                            thread::park();
                        }
                    }

                    match payload.data {
                        None => return Ok(()),
                        Some(v) => value = v,
                    }
                }
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        if self.closed.load(SeqCst) {
            return Err(TryRecvError::Disconnected);
        }

        match self.senders.recv_one() {
            Ok(v) => return Ok(v),
            Err(()) => return Err(TryRecvError::Empty),
        }
    }

    pub fn recv_until(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
        loop {
            match self.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Disconnected) => {
                    return Err(RecvTimeoutError::Disconnected);
                }
                Err(TryRecvError::Empty) => {
                    let mut payload = Payload {
                        data: None,
                        available: ptr::null_mut(),
                    };
                    {
                        let token = self.receivers.register_with(&mut payload);

                        if let Some(end) = deadline {
                            let now = Instant::now();

                            if now >= end {
                                drop(token);

                                match payload.data {
                                    None => return Err(RecvTimeoutError::Timeout),
                                    Some(v) => return Ok(v),
                                }
                                return Err(RecvTimeoutError::Timeout);
                            } else {
                                thread::park_timeout(end - now);
                            }
                        } else {
                            thread::park();
                        }
                    }

                    if let Some(v) = payload.data {
                        return Ok(v);
                    }
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
