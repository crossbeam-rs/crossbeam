//! Channel implementation based on an array.
//!
//! This flavor has a fixed capacity (a positive number).

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::time::Instant;

use crossbeam_utils::cache_padded::CachePadded;

use CaseId;
use actor;
use backoff::Backoff;
use err::{RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};
use monitor::Monitor;

/// An entry in the queue.
///
/// Entries are empty on even laps and hold values on odd laps.
struct Entry<T> {
    /// The current lap.
    ///
    /// Entries are ready for writing on even laps and ready for reading on odd laps.
    lap: AtomicUsize,

    /// The value in this entry.
    value: UnsafeCell<T>,
}

/// An array-based channel with fixed capacity.
///
/// The implementation is based on Dmitry Vyukov's bounded MPMC queue:
///
/// * http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue
/// * https://docs.google.com/document/d/1yIAYmbvL3JxOKOjuCyon7JhW4cSv1wy5hC0ApeGMV9s/pub
pub struct Channel<T> {
    /// Head of the queue (the next index to read from).
    ///
    /// The lower `log2(power)` bits hold the index into the buffer, and the upper bits hold the
    /// current lap, which is always odd for `head`.
    head: CachePadded<AtomicUsize>,

    /// Tail of the queue (the next index to write to).
    ///
    /// The lower `log2(power)` bits hold the index into the buffer, and the upper bits hold the
    /// current lap, which is always even for `tail`.
    tail: CachePadded<AtomicUsize>,

    /// Buffer holding entries in the queue.
    buffer: *mut Entry<T>,

    /// Channel capacity.
    cap: usize,

    /// The next power of two greater than or equal to the capacity.
    power: usize,

    /// Equals `true` if the queue is closed.
    closed: AtomicBool,

    /// Senders waiting on full queue.
    senders: Monitor,

    /// Receivers waiting on empty queue.
    receivers: Monitor,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    /// Returns a new channel with capacity `cap`.
    ///
    /// # Panics
    ///
    /// Panics if the capacity is zero.
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0, "capacity must be positive");

        // Make sure there are at least two most significant bits to encode laps. If this limit is
        // hit, the buffer is most likely too large to allocate anyway.
        let cap_limit = usize::max_value() / 4;
        assert!(
            cap <= cap_limit,
            "channel capacity is too large: {} > {}",
            cap, cap_limit
        );

        // Allocate a buffer of `cap` entries.
        let buffer = {
            let mut v = Vec::<Entry<T>>::with_capacity(cap);
            let ptr = v.as_mut_ptr();
            mem::forget(v);
            ptr
        };

        // Initialize all laps in entries with zero.
        for i in 0..cap {
            unsafe {
                let entry = buffer.offset(i as isize);
                ptr::write(&mut (*entry).lap, AtomicUsize::new(0));
            }
        }

        // Head is initialized with `power` (lap 1, index 0).
        // Tail is initialized with 0 (lap 0, index 0).
        let power = cap.next_power_of_two();
        let head = power;
        let tail = 0;

        Channel {
            buffer,
            cap,
            power,
            head: CachePadded::new(AtomicUsize::new(head)),
            tail: CachePadded::new(AtomicUsize::new(tail)),
            closed: AtomicBool::new(false),
            senders: Monitor::new(),
            receivers: Monitor::new(),
            _marker: PhantomData,
        }
    }

    /// Returns a reference to the entry at `index`.
    ///
    /// # Safety
    ///
    /// The index must be valid, i.e. less than the capacity.
    #[inline]
    unsafe fn entry_at(&self, index: usize) -> &Entry<T> {
        unsafe { &*self.buffer.offset(index as isize) }
    }

    /// Attempts to push `value` into the queue.
    ///
    /// Returns `None` on success, and `Some(value)` if the queue is full.
    fn push(&self, value: T, backoff: &mut Backoff) -> Option<T> {
        loop {
            // Load the tail.
            let tail = self.tail.load(SeqCst);
            let index = tail & (self.power - 1);
            let lap = tail & !(self.power - 1);

            // Inspect the corresponding entry.
            let entry = unsafe { self.entry_at(index) };
            let elap = entry.lap.load(SeqCst);
            let next_elap = elap.wrapping_add(self.power);

            // If the laps of the tail and the entry match, we may attempt to push.
            if lap == elap {
                let new_tail = if index + 1 < self.cap {
                    // Same lap; incremented index.
                    tail + 1
                } else {
                    // Two laps forward; index wraps around to zero.
                    lap.wrapping_add(self.power.wrapping_mul(2))
                };

                // Try moving the tail one entry forward.
                if self.tail
                    .compare_exchange_weak(tail, new_tail, SeqCst, Relaxed)
                    .is_ok()
                {
                    // Write the value into the entry and increment the lap.
                    unsafe { ptr::write(entry.value.get(), value) }
                    entry.lap.store(next_elap, Release);
                    return None;
                }
            // But if the entry lags one lap behind the tail...
            } else if next_elap == lap {
                let head = self.head.load(SeqCst);
                // ...and if head lags one lap behind tail as well...
                if head.wrapping_add(self.power) == tail {
                    // println!("foo");
                    // ...then the queue is full.
                    return Some(value);
                }
            }

            backoff.step();
        }
    }

    /// Attempts to pop a value from the queue.
    ///
    /// Returns `None` if the queue is empty.
    fn pop(&self, backoff: &mut Backoff) -> Option<T> {
        loop {
            // Load the head.
            let head = self.head.load(SeqCst);
            let index = head & (self.power - 1);
            let lap = head & !(self.power - 1);

            // Inspect the corresponding entry.
            let entry = unsafe { self.entry_at(index) };
            let elap = entry.lap.load(SeqCst);
            let next_elap = elap.wrapping_add(self.power);

            // If the laps of the head and the entry match, we may attempt to pop.
            if lap == elap {
                let new = if index + 1 < self.cap {
                    // Same lap; incremented index.
                    head + 1
                } else {
                    // Two laps forward; index wraps around to zero.
                    lap.wrapping_add(self.power.wrapping_mul(2))
                };

                // Try moving the head one entry forward.
                if self.head
                    .compare_exchange_weak(head, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    // Read the value from the entry and increment the lap.
                    let value = unsafe { ptr::read(entry.value.get()) };
                    entry.lap.store(next_elap, Release);
                    return Some(value);
                }
            // But if the entry lags one lap behind the head...
            } else if next_elap == lap {
                let tail = self.tail.load(SeqCst);
                // ...and if tail lags one lap behind head as well...
                if tail.wrapping_add(self.power) == head {
                    // ...then the queue is empty.
                    return None;
                }
            }

            backoff.step();
        }
    }

    /// Returns the current number of values inside the channel.
    pub fn len(&self) -> usize {
        loop {
            // Load the tail, then load the head.
            let tail = self.tail.load(SeqCst);
            let head = self.head.load(SeqCst);

            // If the tail didn't change, we've got consistent values to work with.
            if self.tail.load(SeqCst) == tail {
                let hix = head & (self.power - 1);
                let tix = tail & (self.power - 1);

                return if hix < tix {
                    tix - hix
                } else if hix > tix {
                    self.cap - hix + tix
                } else if tail.wrapping_add(self.power) == head {
                    0
                } else {
                    self.cap
                };
            }
        }
    }

    /// Attempts to send `value` into the channel.
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            match self.push(value, &mut Backoff::new()) {
                None => {
                    self.receivers.notify_one();
                    Ok(())
                }
                Some(v) => Err(TrySendError::Full(v)),
            }
        }
    }

    /// Attempts to send `value` into the channel, retrying several times if it is full.
    pub fn spin_try_send(&self, mut value: T) -> Result<(), TrySendError<T>> {
        if self.closed.load(SeqCst) {
            Err(TrySendError::Disconnected(value))
        } else {
            let backoff = &mut Backoff::new();
            loop {
                match self.push(value, backoff) {
                    None => {
                        self.receivers.notify_one();
                        return Ok(());
                    }
                    Some(v) => value = v,
                }
                if !backoff.step() {
                    break;
                }
            }
            Err(TrySendError::Full(value))
        }
    }

    /// Attempts to send `value` into the channel until the specified `deadline`.
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
            let is_closed = self.is_closed();
            let timed_out = !is_closed && self.is_full() && !actor::current_wait_until(deadline);
            self.senders.unregister(case_id);

            if is_closed {
                return Err(SendTimeoutError::Disconnected(value));
            } else if timed_out {
                return Err(SendTimeoutError::Timeout(value));
            }
        }
    }

    /// Attempts to receive a value from channel.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let closed = self.closed.load(SeqCst);
        match self.pop(&mut Backoff::new()) {
            Some(v) => {
                self.senders.notify_one();
                Ok(v)
            }
            None => if closed {
                Err(TryRecvError::Disconnected)
            } else {
                Err(TryRecvError::Empty)
            },
        }
    }

    /// Attempts to receive a value from channel, retrying several times if it is empty.
    pub fn spin_try_recv(&self) -> Result<T, TryRecvError> {
        let backoff = &mut Backoff::new();
        loop {
            let closed = self.closed.load(SeqCst);
            if let Some(v) = self.pop(backoff) {
                self.senders.notify_one();
                return Ok(v);
            }
            if closed {
                return Err(TryRecvError::Disconnected);
            }
            if !backoff.step() {
                break;
            }
        }
        Err(TryRecvError::Empty)
    }

    /// Attempts to receive a value from the channel until the specified `deadline`.
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
            let is_closed = self.is_closed();
            let timed_out = !is_closed && self.is_empty() && !actor::current_wait_until(deadline);
            self.receivers.unregister(case_id);

            if is_closed && self.is_empty() {
                return Err(RecvTimeoutError::Disconnected);
            } else if timed_out {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    /// Returns the capacity of the channel.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Closes the channel.
    pub fn close(&self) -> bool {
        if self.closed.swap(true, SeqCst) {
            false
        } else {
            self.senders.abort_all();
            self.receivers.abort_all();
            true
        }
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(SeqCst)
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns `true` if the channel is full.
    pub fn is_full(&self) -> bool {
        self.len() == self.cap
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
        let head = self.head.load(Relaxed) & (self.power - 1);

        // Loop over all entries that hold a value and drop them.
        for i in 0..self.len() {
            let index = if head + i < self.cap {
                head + i
            } else {
                head + i - self.cap
            };

            unsafe {
                let entry = self.buffer.offset(index as isize);
                ptr::drop_in_place(entry);
            }
        }

        // Finally, deallocate the buffer, but don't run any destructors.
        unsafe {
            Vec::from_raw_parts(self.buffer, 0, self.cap);
        }
    }
}
