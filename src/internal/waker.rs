//! Waking mechanism for threads blocked on channel operations.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use internal::context::{self, Context};
use internal::select::{Operation, Select};

/// Represents a thread blocked on a specific channel operation.
pub struct Entry {
    /// A context associated with the thread owning this operation.
    pub context: Arc<Context>,

    /// The operation.
    pub oper: Operation,

    /// Optional packet.
    pub packet: usize,
}

/// Queue of threads blocked on channel operations.
///
/// This data structure is used by threads to registering blocking operations and get woken up once
/// an operation becomes ready.
pub struct Waker {
    /// List of registered threads blocked on channel operations.
    entries: VecDeque<Entry>,
}

impl Waker {
    /// Creates a new `Waker`.
    #[inline]
    pub fn new() -> Self {
        Waker {
            entries: VecDeque::new(),
        }
    }

    /// Registers the current thread with an operation.
    #[inline]
    pub fn register(&mut self, oper: Operation) {
        self.register_with_packet(oper, 0);
    }

    /// Registers the current thread with an operation and packe.
    #[inline]
    pub fn register_with_packet(&mut self, oper: Operation, packet: usize) {
        self.entries.push_back(Entry {
            context: context::current(),
            oper,
            packet,
        });
    }

    /// Unregisters the current thread with an operation.
    #[inline]
    pub fn unregister(&mut self, oper: Operation) -> Option<Entry> {
        if let Some((i, _)) = self.entries
            .iter()
            .enumerate()
            .find(|&(_, entry)| entry.oper == oper)
        {
            let entry = self.entries.remove(i);
            Self::maybe_shrink(&mut self.entries);
            entry
        } else {
            None
        }
    }

    /// Attempts to find one thread (not the current one), select its operation, and wake it up.
    #[inline]
    pub fn wake_one(&mut self) -> Option<Entry> {
        if !self.entries.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.entries.len() {
                // Does the entry belong to a different thread?
                if self.entries[i].context.thread_id() != thread_id {
                    // Try selecting this operation.
                    let sel = Select::Operation(self.entries[i].oper);
                    let res = self.entries[i].context.try_select(sel);

                    if res.is_ok() {
                        // Provide the packet, too.
                        self.entries[i].context.store_packet(self.entries[i].packet);

                        // Remove the entry from the queue to improve performance.
                        let entry = self.entries.remove(i).unwrap();
                        Self::maybe_shrink(&mut self.entries);

                        // Wake the thread up.
                        entry.context.unpark();
                        return Some(entry);
                    }
                }
            }
        }

        None
    }

    /// Notifies all threads that the channel is closed.
    #[inline]
    pub fn close(&mut self) {
        for entry in self.entries.iter() {
            if entry.context.try_select(Select::Closed).is_ok() {
                // Wake the thread up.
                //
                // Here we don't remove the entry from the queue. Registered threads might want to
                // unregister from the waker themselves in order to recover the packet value and
                // destroy the packet, if necessary.
                entry.context.unpark();
            }
        }
    }

    /// Returns `true` if there is an entry which can be woken up by the current thread.
    #[inline]
    pub fn can_wake_one(&self) -> bool {
        if !self.entries.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.entries.len() {
                if self.entries[i].context.thread_id() != thread_id
                    && self.entries[i].context.selected() == Select::Waiting
                {
                    return true;
                }
            }
        }
        false
    }

    /// Returns the number of entries in the queue.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Shrinks the internal queue if it's capacity is much larger than length.
    #[inline]
    fn maybe_shrink(entries: &mut VecDeque<Entry>) {
        if entries.capacity() > 32 && entries.len() < entries.capacity() / 4 {
            let mut v = VecDeque::with_capacity(entries.capacity() / 2);
            v.extend(entries.drain(..));
            *entries = v;
        }
    }
}

impl Drop for Waker {
    #[inline]
    fn drop(&mut self) {
        debug_assert!(self.entries.is_empty());
    }
}

/// A simple wait queue for list-based and array-based channels.
///
/// This data structure is used for registering select operations before blocking and waking them
/// up when the channel receives a message, sends one, or gets closed.
pub struct SyncWaker {
    /// The list of registered select operations.
    inner: Mutex<Waker>,

    /// Number of operations in the list.
    len: AtomicUsize,
}

impl SyncWaker {
    /// Creates a new `SyncWaker`.
    #[inline]
    pub fn new() -> Self {
        SyncWaker {
            inner: Mutex::new(Waker::new()),
            len: AtomicUsize::new(0),
        }
    }

    /// Registers the current thread with an operation.
    #[inline]
    pub fn register(&self, oper: Operation) {
        let mut inner = self.inner.lock();
        inner.register(oper);
        self.len.store(inner.len(), Ordering::SeqCst);
    }

    /// Unregisters the current thread with an operation.
    #[inline]
    pub fn unregister(&self, oper: Operation) -> Option<Entry> {
        if self.len.load(Ordering::SeqCst) > 0 {
            let mut inner = self.inner.lock();
            let entry = inner.unregister(oper);
            self.len.store(inner.len(), Ordering::SeqCst);
            entry
        } else {
            None
        }
    }

    /// Attempts to find one thread (not the current one), select its operation, and wake it up.
    #[inline]
    pub fn wake_one(&self) -> Option<Entry> {
        if self.len.load(Ordering::SeqCst) > 0 {
            let mut inner = self.inner.lock();
            let entry = inner.wake_one();
            self.len.store(inner.len(), Ordering::SeqCst);
            entry
        } else {
            None
        }
    }

    /// Notifies all threads that the channel is closed.
    pub fn close(&self) {
        self.inner.lock().close();
    }
}

impl Drop for SyncWaker {
    #[inline]
    fn drop(&mut self) {
        debug_assert_eq!(self.inner.lock().len(), 0);
        debug_assert_eq!(self.len.load(Ordering::SeqCst), 0);
    }
}
