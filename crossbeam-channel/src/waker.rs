//! Waking mechanism for threads blocked on channel operations.

use std::collections::VecDeque;
use std::num::Wrapping;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, ThreadId};

use parking_lot::Mutex;

use context::Context;
use select::{Operation, Selected};

/// Represents a thread blocked on a specific channel operation.
pub struct Entry {
    /// Context associated with the thread owning this operation.
    pub context: Context,

    /// The operation.
    pub oper: Operation,

    /// Optional packet.
    pub packet: usize,
}

/// A queue of threads blocked on channel operations.
///
/// This data structure is used by threads to register blocking operations and get woken up once
/// an operation becomes ready.
pub struct Waker {
    /// The list of registered blocking operations.
    entries: VecDeque<Entry>,

    /// The number of calls to `register` and `register_with_packet`.
    register_count: Wrapping<usize>,
}

impl Waker {
    /// Creates a new `Waker`.
    #[inline]
    pub fn new() -> Self {
        Waker {
            entries: VecDeque::new(),
            register_count: Wrapping(0),
        }
    }

    /// Registers the current thread with an operation.
    #[inline]
    pub fn register(&mut self, oper: Operation, cx: &Context) {
        self.register_with_packet(oper, 0, cx);
    }

    /// Registers the current thread with an operation and a packet.
    #[inline]
    pub fn register_with_packet(&mut self, oper: Operation, packet: usize, cx: &Context) {
        self.entries.push_back(Entry {
            context: cx.clone(),
            oper,
            packet,
        });
        self.register_count += Wrapping(1);
    }

    /// Unregisters an operation previously registered by the current thread.
    #[inline]
    pub fn unregister(&mut self, oper: Operation) -> Option<Entry> {
        if let Some((i, _)) = self
            .entries
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
            let thread_id = current_thread_id();

            for i in 0..self.entries.len() {
                // Does the entry belong to a different thread?
                if self.entries[i].context.thread_id() != thread_id {
                    // Try selecting this operation.
                    let sel = Selected::Operation(self.entries[i].oper);
                    let res = self.entries[i].context.try_select(sel);

                    if res.is_ok() {
                        // Provide the packet.
                        self.entries[i].context.store_packet(self.entries[i].packet);
                        // Wake the thread up.
                        self.entries[i].context.unpark();

                        // Remove the entry from the queue to keep it clean and improve
                        // performance.
                        let entry = self.entries.remove(i).unwrap();
                        Self::maybe_shrink(&mut self.entries);
                        return Some(entry);
                    }
                }
            }
        }

        None
    }

    /// Notifies all threads that the channel is disconnected.
    #[inline]
    pub fn disconnect(&mut self) {
        for entry in self.entries.iter() {
            if entry.context.try_select(Selected::Disconnected).is_ok() {
                // Wake the thread up.
                //
                // Here we don't remove the entry from the queue. Registered threads must
                // unregister from the waker by themselves. They might also want to recover the
                // packet value and destroy it, if necessary.
                entry.context.unpark();
            }
        }
    }

    /// Returns `true` if there is an entry which can be woken up by the current thread.
    #[inline]
    pub fn can_wake_one(&self) -> bool {
        if self.entries.is_empty() {
            false
        } else {
            let thread_id = current_thread_id();

            self.entries.iter().any(|entry| {
                entry.context.thread_id() != thread_id
                    && entry.context.selected() == Selected::Waiting
            })
        }
    }

    /// Returns the number of entries in the queue.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn register_count(&self) -> usize {
        self.register_count.0
    }

    /// Shrinks the internal queue if its capacity is much larger than length.
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

/// A waker that can be shared among threads without locking.
///
/// This is a simple wrapper around `Waker` that internally uses a mutex for synchronization.
pub struct SyncWaker {
    /// The inner `Waker`.
    inner: Mutex<Waker>,

    /// Number of operations in the waker.
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
    pub fn register(&self, oper: Operation, cx: &Context) {
        let mut inner = self.inner.lock();
        inner.register(oper, cx);
        self.len.store(inner.len(), Ordering::SeqCst);
    }

    /// Unregisters an operation previously registered by the current thread.
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

    /// Notifies all threads that the channel is disconnected.
    pub fn disconnect(&self) {
        self.inner.lock().disconnect();
    }
}

impl Drop for SyncWaker {
    #[inline]
    fn drop(&mut self) {
        debug_assert_eq!(self.inner.lock().len(), 0);
        debug_assert_eq!(self.len.load(Ordering::SeqCst), 0);
    }
}

/// Returns the id of the current thread.
#[inline]
fn current_thread_id() -> ThreadId {
    thread_local! {
        /// Cached thread-local id.
        static THREAD_ID: ThreadId = thread::current().id();
    }

    THREAD_ID
        .try_with(|id| *id)
        .unwrap_or_else(|_| thread::current().id())
}
