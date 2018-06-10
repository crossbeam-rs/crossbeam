//! TODO

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use internal::context::{self, Context};
use internal::select::{Operation, Select};

/// A select operation, identified by a `Context` and a `Select`.
///
/// Note that multiple threads could be operating on a single channel end, as well as a single
/// thread on multiple different channel ends.
pub struct Entry {
    /// A context associated with the thread owning this operation.
    pub context: Arc<Context>,

    /// The operation.
    pub oper: Operation,

    /// TODO
    pub packet: usize,
}

/// A simple wait queue for zero-capacity channels.
///
/// This data structure is used for registering select operations before blocking and waking them
/// up when the channel receives a message, sends one, or gets closed.
pub struct Waker {
    /// The list of registered select operations.
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

    #[inline]
    pub fn register(&mut self, oper: Operation) {
        self.register_with_packet(oper, 0);
    }

    #[inline]
    pub fn register_with_packet(&mut self, oper: Operation, packet: usize) {
        self.entries.push_back(Entry {
            context: context::current(),
            oper,
            packet,
        });
    }

    /// Unregisters the current thread with `select`.
    #[inline]
    pub fn unregister(&mut self, oper: Operation) -> Option<Entry> {
        if let Some((i, _)) = self.entries
            .iter()
            .enumerate()
            .find(|&(_, operation)| operation.oper == oper)
        {
            let operation = self.entries.remove(i);
            Self::maybe_shrink(&mut self.entries);
            operation
        } else {
            None
        }
    }

    #[inline]
    pub fn wake_one(&mut self) -> Option<Entry> {
        if !self.entries.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.entries.len() {
                if self.entries[i].context.thread_id() != thread_id {
                    let sel = Select::Operation(self.entries[i].oper);
                    let res = self.entries[i].context.try_select(sel);

                    if res.is_ok() {
                        self.entries[i].context.store_packet(self.entries[i].packet);

                        let operation = self.entries.remove(i).unwrap();
                        Self::maybe_shrink(&mut self.entries);

                        operation.context.unpark();
                        return Some(operation);
                    }
                }
            }
        }

        None
    }

    /// TODO Aborts all registered select operations.
    #[inline]
    pub fn close(&mut self) {
        // TODO: explain why not drain
        for operation in self.entries.iter() {
            if operation.context.try_select(Select::Closed).is_ok() {
                operation.context.unpark();
            }
        }
    }

    /// Returns `true` if there exists a operation which isn't owned by the current thread.
    #[inline]
    // TODO: rename contains_other_threads?
    pub fn can_notify(&self) -> bool {
        if !self.entries.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.entries.len() {
                if self.entries[i].context.thread_id() != thread_id {
                    return true;
                }
            }
        }
        false
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Shrinks the internal deque if it's capacity is much larger than length.
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

    /// Registers the current thread with `select`.
    #[inline]
    pub fn register(&self, oper: Operation) {
        let mut inner = self.inner.lock();
        inner.register(oper);
        self.len.store(inner.len(), Ordering::SeqCst);
    }

    /// Unregisters the current thread with `select`.
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

    /// TODO Aborts all registered select operations.
    pub fn close(&self) {
        self.inner.lock().close();
    }

    /// Returns `true` if there exists an operation which isn't owned by the current thread.
    #[inline]
    pub fn can_notify(&self) -> bool {
        if self.len.load(Ordering::SeqCst) > 0 {
            self.inner.lock().can_notify()
        } else {
            false
        }
    }
}

impl Drop for SyncWaker {
    #[inline]
    fn drop(&mut self) {
        debug_assert_eq!(self.inner.lock().len(), 0);
        debug_assert_eq!(self.len.load(Ordering::SeqCst), 0);
    }
}
