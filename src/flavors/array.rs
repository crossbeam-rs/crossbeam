//! Channel implementation based on an array.
//!
//! This flavor has a fixed, positive capacity.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, Release, SeqCst};
use std::time::Instant;

use crossbeam_utils::cache_padded::CachePadded;

use err::{TryRecvError, TrySendError};
use monitor::Monitor;
use select::CaseId;
use select::handle;
use utils::Backoff;

/// An entry in the channel.
///
/// Entries are empty on even laps and hold messages on odd laps.
struct Entry<T> {
    /// The current lap.
    ///
    /// Entries are ready for writing on even laps and ready for reading on odd laps.
    lap: AtomicUsize,

    /// The message in this entry.
    msg: UnsafeCell<T>,
}

/// The list of possible error outcomes for the `push` operation.
enum PushError<T> {
    /// The channel is full.
    Full(T),

    /// The channel is closed.
    Closed(T),
}

/// The list of possible error outcomes for the `pop` operation.
enum PopError {
    /// The channel is empty.
    Empty,

    /// The channel is closed.
    Closed,
}

/// An array-based channel with fixed capacity.
///
/// The implementation is based on Dmitry Vyukov's bounded MPMC queue:
///
/// * http://www.1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue
/// * https://docs.google.com/document/d/1yIAYmbvL3JxOKOjuCyon7JhW4cSv1wy5hC0ApeGMV9s/pub
pub struct Channel<T> {
    /// Head of the channel (the next index to read from).
    ///
    /// Bits lower than the mark bit represent the index, while the upper bits represent the lap.
    /// The mark bit is always zero.
    head: CachePadded<AtomicUsize>,

    /// Tail of the channel (the next index to write to).
    ///
    /// Bits lower than the mark bit represent the index, while the upper bits represent the lap.
    /// If the mark bit is set, that means the channel is closed and the tail cannot move forward
    /// any further.
    tail: CachePadded<AtomicUsize>,

    /// Buffer holding entries in the channel.
    buffer: *mut Entry<T>,

    /// Channel capacity.
    cap: usize,

    /// The mark bit.
    ///
    /// If the mark bit in the tail is set, that indicates the channel is closed.
    mark_bit: usize,

    /// Senders waiting on full channel.
    senders: Monitor,

    /// Receivers waiting on empty channel.
    receivers: Monitor,

    /// Indicates that dropping a `Channel<T>` may drop values of type `T`.
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    pub fn sel_try_recv(&self) -> Option<usize> {
        match self.sel_pop() {
            Ok(x) => Some(x),
            Err(PopError::Closed) => Some(0),
            Err(PopError::Empty) => None,
        }
    }

    pub unsafe fn finish_recv(&self, token: usize) -> Option<T> {
        if token == 0 {
            None
        } else {
            let one_lap = self.mark_bit << 1;

            unsafe {
                let entry: &Entry<T> = &*(token as *const Entry<T>);

                // Read the message from the entry and increment the lap.
                let msg = ptr::read(entry.msg.get());
                // TODO: Optimize by changing `fetch_add` to `store`
                entry.lap.fetch_add(one_lap, Release);

                self.senders.notify_one();
                Some(msg)
            }
        }
    }

    pub fn sel_try_send(&self) -> Option<usize> {
        let backoff = &mut Backoff::new();
        loop {
            match self.sel_push(backoff) {
                Ok(x) => return Some(x),
                Err(PushError::Closed(())) => unreachable!(), // TODO: delete this case
                Err(PushError::Full(())) => {},
            }

            // TODO:
            if !backoff.step() {
                return None;
            }
        }
    }

    pub unsafe fn finish_send(&self, token: usize, msg: T) {
        let one_lap = self.mark_bit << 1;

        unsafe {
            let entry: &Entry<T> = &*(token as *const Entry<T>);

            // Write the message into the entry and increment the lap.
            ptr::write(entry.msg.get(), msg);
            // TODO: Optimize by changing `fetch_add` to `store`
            entry.lap.fetch_add(one_lap, Release);

            self.receivers.notify_one();
        }
    }

    /// Returns a new channel with capacity `cap`.
    ///
    /// # Panics
    ///
    /// Panics if the capacity is not in the range `1 .. usize::max_value() / (1 << 3)`.
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap > 0, "capacity must be positive");

        // Make sure there are at least two most significant bits to encode laps, plus one more bit
        // for marking the tail to indicate that the channel is closed. If we can't reserve three
        // bits, then panic. In that case, the buffer is likely too large to allocate anyway.
        let cap_limit = usize::max_value() / (1 << 3);
        assert!(
            cap <= cap_limit,
            "channel capacity is too large: {} > {}",
            cap,
            cap_limit
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

        // The mark bit is the smallest power of two greater than or equal to `cap`.
        let mark_bit = cap.next_power_of_two();

        // Head is initialized with (lap: 1, mark: 0, index: 0).
        // Tail is initialized with (lap: 0, mark: 0, index: 0).
        let head = mark_bit << 1;
        let tail = 0;

        Channel {
            buffer,
            cap,
            mark_bit,
            head: CachePadded::new(AtomicUsize::new(head)),
            tail: CachePadded::new(AtomicUsize::new(tail)),
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
        &*self.buffer.offset(index as isize)
    }

    fn sel_push(&self, backoff: &mut Backoff) -> Result<usize, PushError<()>> {
        let one_lap = self.mark_bit << 1;
        let index_bits = self.mark_bit - 1;
        let lap_bits = !(one_lap - 1);

        loop {
            // Load the tail.
            let tail = self.tail.load(SeqCst);

            // If the tail is marked, the channel is closed.
            if tail & self.mark_bit != 0 {
                return Err(PushError::Closed(()));
            }

            let index = tail & index_bits;
            let lap = tail & lap_bits;

            // Inspect the corresponding entry.
            let entry = unsafe { self.entry_at(index) };
            let elap = entry.lap.load(SeqCst);
            let next_elap = elap.wrapping_add(one_lap);

            // If the laps of the tail and the entry match, we may attempt to push.
            if lap == elap {
                let new_tail = if index + 1 < self.cap {
                    // Same lap; incremented index.
                    tail + 1
                } else {
                    // Two laps forward; index wraps around to zero.
                    lap.wrapping_add(one_lap.wrapping_mul(2))
                };

                // Try moving the tail one entry forward.
                if self.tail
                    .compare_exchange_weak(tail, new_tail, SeqCst, Relaxed)
                    .is_ok()
                {
                    return Ok(entry as *const Entry<T> as usize);
                }
            // But if the entry lags one lap behind the tail...
            } else if next_elap == lap {
                let head = self.head.load(SeqCst);

                // ...and if head lags one lap behind tail as well...
                if head.wrapping_add(one_lap) == tail {
                    // ...then the channel is full.
                    return Err(PushError::Full(()));
                }
            }

            backoff.step();
        }
    }

    /// Attempts to push `msg` into the channel.
    ///
    /// Returns `None` on success, and `Some(msg)` if the channel is full.
    fn push(&self, msg: T, backoff: &mut Backoff) -> Result<(), PushError<T>> {
        let one_lap = self.mark_bit << 1;
        let index_bits = self.mark_bit - 1;
        let lap_bits = !(one_lap - 1);

        loop {
            // Load the tail.
            let tail = self.tail.load(SeqCst);

            // If the tail is marked, the channel is closed.
            if tail & self.mark_bit != 0 {
                return Err(PushError::Closed(msg));
            }

            let index = tail & index_bits;
            let lap = tail & lap_bits;

            // Inspect the corresponding entry.
            let entry = unsafe { self.entry_at(index) };
            let elap = entry.lap.load(SeqCst);
            let next_elap = elap.wrapping_add(one_lap);

            // If the laps of the tail and the entry match, we may attempt to push.
            if lap == elap {
                let new_tail = if index + 1 < self.cap {
                    // Same lap; incremented index.
                    tail + 1
                } else {
                    // Two laps forward; index wraps around to zero.
                    lap.wrapping_add(one_lap.wrapping_mul(2))
                };

                // Try moving the tail one entry forward.
                if self.tail
                    .compare_exchange_weak(tail, new_tail, SeqCst, Relaxed)
                    .is_ok()
                {
                    // Write the message into the entry and increment the lap.
                    unsafe { ptr::write(entry.msg.get(), msg) }
                    entry.lap.store(next_elap, Release);
                    return Ok(());
                }
            // But if the entry lags one lap behind the tail...
            } else if next_elap == lap {
                let head = self.head.load(SeqCst);

                // ...and if head lags one lap behind tail as well...
                if head.wrapping_add(one_lap) == tail {
                    // ...then the channel is full.
                    return Err(PushError::Full(msg));
                }
            }

            backoff.step();
        }
    }

    fn sel_pop(&self) -> Result<usize, PopError> {
        let one_lap = self.mark_bit << 1;
        let index_bits = self.mark_bit - 1;
        let lap_bits = !(one_lap - 1);

        let mut backoff = Backoff::new();

        loop {
            // Load the head.
            let head = self.head.load(SeqCst);
            let index = head & index_bits;
            let lap = head & lap_bits;

            // Inspect the corresponding entry.
            let entry = unsafe { self.entry_at(index) };
            let elap = entry.lap.load(SeqCst);
            let next_elap = elap.wrapping_add(one_lap);

            // If the laps of the head and the entry match, we may attempt to pop.
            if lap == elap {
                let new = if index + 1 < self.cap {
                    // Same lap; incremented index.
                    head + 1
                } else {
                    // Two laps forward; index wraps around to zero.
                    lap.wrapping_add(one_lap.wrapping_mul(2))
                };

                // Try moving the head one entry forward.
                if self.head
                    .compare_exchange_weak(head, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    return Ok(entry as *const Entry<T> as usize);
                }
            // But if the entry lags one lap behind the head...
            } else if next_elap == lap {
                let tail = self.tail.load(SeqCst);

                // ...and if the tail lags one lap behind the head as well, that means the channel
                // is empty.
                if (tail & !self.mark_bit).wrapping_add(one_lap) == head {
                    // Check whether the channel is closed and return the appropriate error
                    // variant.
                    if tail & self.mark_bit != 0 {
                        return Err(PopError::Closed);
                    } else {
                        return Err(PopError::Empty);
                    }
                }
            }

            backoff.step();
        }
    }

    /// Attempts to pop a message from the channel.
    ///
    /// Returns `None` if the channel is empty.
    fn pop(&self, backoff: &mut Backoff) -> Result<T, PopError> {
        let one_lap = self.mark_bit << 1;
        let index_bits = self.mark_bit - 1;
        let lap_bits = !(one_lap - 1);

        loop {
            // Load the head.
            let head = self.head.load(SeqCst);
            let index = head & index_bits;
            let lap = head & lap_bits;

            // Inspect the corresponding entry.
            let entry = unsafe { self.entry_at(index) };
            let elap = entry.lap.load(SeqCst);
            let next_elap = elap.wrapping_add(one_lap);

            // If the laps of the head and the entry match, we may attempt to pop.
            if lap == elap {
                let new = if index + 1 < self.cap {
                    // Same lap; incremented index.
                    head + 1
                } else {
                    // Two laps forward; index wraps around to zero.
                    lap.wrapping_add(one_lap.wrapping_mul(2))
                };

                // Try moving the head one entry forward.
                if self.head
                    .compare_exchange_weak(head, new, SeqCst, Relaxed)
                    .is_ok()
                {
                    // Read the message from the entry and increment the lap.
                    let msg = unsafe { ptr::read(entry.msg.get()) };
                    entry.lap.store(next_elap, Release);
                    return Ok(msg);
                }
            // But if the entry lags one lap behind the head...
            } else if next_elap == lap {
                let tail = self.tail.load(SeqCst);

                // ...and if the tail lags one lap behind the head as well, that means the channel
                // is empty.
                if (tail & !self.mark_bit).wrapping_add(one_lap) == head {
                    // Check whether the channel is closed and return the appropriate error
                    // variant.
                    if tail & self.mark_bit != 0 {
                        return Err(PopError::Closed);
                    } else {
                        return Err(PopError::Empty);
                    }
                }
            }

            backoff.step();
        }
    }

    /// Returns the current number of messages inside the channel.
    pub fn len(&self) -> usize {
        let one_lap = self.mark_bit << 1;
        let index_bits = self.mark_bit - 1;

        loop {
            // Load the tail, then load the head.
            let tail = self.tail.load(SeqCst);
            let head = self.head.load(SeqCst);

            // If the tail didn't change, we've got consistent values to work with.
            if self.tail.load(SeqCst) == tail {
                // Clear out the mark bit, just in case it is set.
                let tail = tail & !self.mark_bit;

                let hix = head & index_bits;
                let tix = tail & index_bits;

                return if hix < tix {
                    tix - hix
                } else if hix > tix {
                    self.cap - hix + tix
                } else if tail.wrapping_add(one_lap) == head {
                    0
                } else {
                    self.cap
                };
            }
        }
    }

    /// Attempts to send `msg` into the channel.
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        match self.push(msg, &mut Backoff::new()) {
            Ok(()) => {
                self.receivers.notify_one();
                Ok(())
            }
            Err(PushError::Full(m)) => Err(TrySendError::Full(m)),
            Err(PushError::Closed(m)) => Err(TrySendError::Closed(m)),
        }
    }

    /// Attempts to send `msg` into the channel.
    pub fn send(&self, mut msg: T, case_id: CaseId) {
        loop {
            let backoff = &mut Backoff::new();
            loop {
                match self.push(msg, backoff) {
                    Ok(()) => {
                        self.receivers.notify_one();
                        return;
                    }
                    Err(PushError::Full(m)) => msg = m,
                    Err(PushError::Closed(m)) => panic!(), // TODO: delete this
                }

                if !backoff.step() {
                    break;
                }
            }

            handle::current_reset();
            self.senders.register(case_id);
            if self.is_full() {
                handle::current_wait_until(None);
            }
            self.senders.unregister(case_id);
        }
    }

    /// Attempts to receive a message from channel.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.pop(&mut Backoff::new()) {
            Ok(m) => {
                self.senders.notify_one();
                Ok(m)
            }
            Err(PopError::Empty) => Err(TryRecvError::Empty),
            Err(PopError::Closed) => Err(TryRecvError::Closed),
        }
    }

    /// Attempts to receive a message from the channel.
    pub fn recv(&self, case_id: CaseId) -> Option<T> {
        loop {
            let backoff = &mut Backoff::new();
            loop {
                match self.pop(backoff) {
                    Ok(m) => {
                        self.senders.notify_one();
                        return Some(m);
                    }
                    Err(PopError::Empty) => {},
                    Err(PopError::Closed) => return None,
                }

                if !backoff.step() {
                    break;
                }
            }

            handle::current_reset();
            self.receivers.register(case_id);
            if !self.is_closed() && self.is_empty() {
                handle::current_wait_until(None);
            }
            self.receivers.unregister(case_id);
        }
    }

    /// Returns the capacity of the channel.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Closes the channel and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        let tail = self.tail.fetch_or(self.mark_bit, SeqCst);

        // Was the channel already closed?
        if tail & self.mark_bit != 0 {
            false
        } else {
            self.senders.abort_all();
            self.receivers.abort_all();
            true
        }
    }

    /// Returns `true` if the channel is closed.
    pub fn is_closed(&self) -> bool {
        self.tail.load(SeqCst) & self.mark_bit != 0
    }

    /// Returns `true` if the channel is empty.
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(SeqCst);
        let tail = self.tail.load(SeqCst) & !self.mark_bit;

        // Is the tail lagging one lap behind head?
        let one_lap = self.mark_bit << 1;
        tail.wrapping_add(one_lap) == head
    }

    /// Returns `true` if the channel is full.
    pub fn is_full(&self) -> bool {
        let tail = self.tail.load(SeqCst) & !self.mark_bit;
        let head = self.head.load(SeqCst);

        // Is the head lagging one lap behind tail?
        let one_lap = self.mark_bit << 1;
        head.wrapping_add(one_lap) == tail
    }

    /// Returns a reference to the monitor for this channel's senders.
    pub fn senders(&self) -> &Monitor {
        &self.senders
    }

    /// Returns a reference to the monitor for this channel's receivers.
    pub fn receivers(&self) -> &Monitor {
        &self.receivers
    }
}

impl<T> Drop for Channel<T> {
    fn drop(&mut self) {
        let index_bits = self.mark_bit - 1;
        let head = self.head.load(Relaxed) & index_bits;

        // Loop over all entries that hold a message and drop them.
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
