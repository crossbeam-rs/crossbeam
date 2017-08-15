// Based on Dmitry Vyukov's MPMC queue:
// http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue
// https://docs.google.com/document/d/1yIAYmbvL3JxOKOjuCyon7JhW4cSv1wy5hC0ApeGMV9s/pub

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::thread;
use std::time::Instant;

use CaseId;
use actor;
use err::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};
use monitor::Monitor;

struct Node<T> {
    lap: AtomicUsize,
    value: T,
}

#[repr(C)]
pub(crate) struct Channel<T> {
    buffer: *mut UnsafeCell<Node<T>>,
    cap: usize,
    power: usize,
    _pad0: [u8; 64],
    head: AtomicUsize,
    _pad1: [u8; 64],
    tail: AtomicUsize,
    _pad2: [u8; 64],
    closed: AtomicBool,
    senders: Monitor,
    receivers: Monitor,
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0);
        assert!(cap <= ::std::isize::MAX as usize);

        let power = cap.next_power_of_two();

        let mut v = Vec::with_capacity(cap);
        let buffer = v.as_mut_ptr();
        mem::forget(v);

        unsafe {
            ptr::write_bytes(buffer, 0, cap);
        }

        Channel {
            buffer: buffer,
            cap: cap,
            power: power,
            head: AtomicUsize::new(power),
            tail: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            senders: Monitor::new(),
            receivers: Monitor::new(),
            _pad0: [0; 64],
            _pad1: [0; 64],
            _pad2: [0; 64],
            _marker: PhantomData,
        }
    }

    fn push(&self, value: T, spinwait: &mut Spinwait) -> Result<(), T> {
        let cap = self.cap;
        let power = self.power;
        let buffer = self.buffer;

        loop {
            let tail = self.tail.load(SeqCst);
            let pos = tail & (power - 1);
            let lap = tail & !(power - 1);

            let cell = unsafe { (*buffer.offset(pos as isize)).get() };
            let clap = unsafe { (*cell).lap.load(SeqCst) };

            if lap == clap {
                let new = if pos + 1 < cap {
                    tail + 1
                } else {
                    lap.wrapping_add(power).wrapping_add(power)
                };

                if self.tail
                    .compare_exchange_weak(tail, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    unsafe {
                        ptr::write(&mut (*cell).value, value);
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Ok(());
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return Err(value);
            }

            if !spinwait.spin() {
                thread::yield_now();
            }
        }
    }

    fn pop(&self, spinwait: &mut Spinwait) -> Result<T, ()> {
        let cap = self.cap;
        let power = self.power;
        let buffer = self.buffer;

        loop {
            let head = self.head.load(SeqCst);
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

                if self.head
                    .compare_exchange_weak(head, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    unsafe {
                        let value = ptr::read(&(*cell).value);
                        (*cell).lap.store(clap.wrapping_add(power), Release);
                        return Ok(value);
                    }
                }
            } else if clap.wrapping_add(power) == lap {
                return Err(());
            }

            if !spinwait.spin() {
                thread::yield_now();
            }
        }
    }

    pub fn len(&self) -> usize {
        let cap = self.cap;
        let power = self.power;

        loop {
            let tail = self.tail.load(SeqCst);
            let head = self.head.load(SeqCst);

            if self.tail.load(SeqCst) == tail {
                let tpos = tail & (power - 1);
                let tlap = tail & !(power - 1);

                let hpos = head & (power - 1);
                let hlap = head & !(power - 1);

                if hlap.wrapping_add(power) == tlap {
                    debug_assert!(cap - hpos + tpos <= cap);
                    return cap - hpos + tpos;
                } else {
                    debug_assert!(tpos >= hpos && tpos - hpos <= cap);
                    return tpos - hpos;
                }
            }
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            match self.push(value, &mut Spinwait::new()) {
                Ok(()) => {
                    self.receivers.notify_one();
                    Ok(())
                }
                Err(v) => Err(TrySendError::Full(v)),
            }
        }
    }

    pub fn spin_try_send(&self, mut value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            let spinwait = &mut Spinwait::new();
            loop {
                match self.push(value, spinwait) {
                    Ok(()) => {
                        self.receivers.notify_one();
                        return Ok(());
                    }
                    Err(v) => value = v,
                }
                if !spinwait.spin() {
                    break;
                }
            }
            Err(TrySendError::Full(value))
        }
    }

    pub fn send_until(
        &self,
        mut value: T,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<(), SendTimeoutError<T>> {
        loop {
            match self.spin_try_send(value) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(v)) => value = v,
                Err(TrySendError::Disconnected(v)) => return Err(SendTimeoutError::Disconnected(v)),
            }

            actor::current_reset();
            self.senders.register(case_id);
            let timed_out =
                !self.is_closed() && self.len() == self.cap && !actor::current_wait_until(deadline);
            self.senders.unregister(case_id);

            if timed_out {
                return Err(SendTimeoutError::Timeout(value));
            }
        }
    }

    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let closed = self.closed.load(SeqCst);
        match self.pop(&mut Spinwait::new()) {
            Ok(v) => {
                self.senders.notify_one();
                Ok(v)
            }
            Err(()) => if closed {
                Err(TryRecvError::Disconnected)
            } else {
                Err(TryRecvError::Empty)
            },
        }
    }

    pub fn spin_try_recv(&self) -> Result<T, TryRecvError> {
        let spinwait = &mut Spinwait::new();
        loop {
            let closed = self.closed.load(SeqCst);
            if let Ok(v) = self.pop(spinwait) {
                self.senders.notify_one();
                return Ok(v);
            }
            if closed {
                return Err(TryRecvError::Disconnected);
            }
            if !spinwait.spin() {
                break;
            }
        }
        Err(TryRecvError::Empty)
    }

    pub fn recv_until(
        &self,
        deadline: Option<Instant>,
        case_id: CaseId,
    ) -> Result<T, RecvTimeoutError> {
        loop {
            match self.spin_try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
            }

            actor::current_reset();
            self.receivers.register(case_id);
            let timed_out =
                !self.is_closed() && self.len() == 0 && !actor::current_wait_until(deadline);
            self.receivers.unregister(case_id);

            if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            false
        } else {
            self.senders.abort_all();
            self.receivers.abort_all();
            true
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    pub fn senders(&self) -> &Monitor {
        &self.senders
    }

    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        let cap = self.cap;
        let power = self.power;
        let buffer = self.buffer;

        let head = self.head.load(Relaxed);
        let mut pos = head & (power - 1);

        unsafe {
            for _ in 0..self.len() {
                let cell = (*buffer.offset(pos as isize)).get();
                ptr::drop_in_place(cell);

                pos += 1;
                if pos == self.cap {
                    pos = 0;
                }
            }

            Vec::from_raw_parts(buffer, 0, cap);
        }
    }
}

struct Spinwait(u32);

impl Spinwait {
    #[inline]
    fn new() -> Self {
        Spinwait(0)
    }

    #[inline]
    fn spin(&mut self) -> bool {
        if self.0 >= 20 {
            false
        } else {
            if self.0 < 10 {
                for _ in 0..1 << self.0 {
                    // ::std::sync::atomic::hint_core_should_pause();
                }
            } else {
                thread::yield_now();
            }

            self.0 += 1;
            true
        }
    }
}
