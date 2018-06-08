use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use internal::context::{self, Context};
use internal::select::Select;

/// A select operation, identified by a `Context` and a `Select`.
///
/// Note that multiple threads could be operating on a single channel end, as well as a single
/// thread on multiple different channel ends.
pub struct Operation {
    /// A context associated with the thread owning this operation.
    pub context: Arc<Context>,

    /// The operation ID.
    pub select: Select,
}

/// A simple wait queue for list-based and array-based channels.
///
/// This data structure is used for registering selection operations before blocking and waking
/// them up when the channel receives a message, sends one, or gets closed.
pub struct SyncWaker {
    /// The list of registered selection operations.
    operations: Mutex<VecDeque<Operation>>,

    /// Number of operations in the list.
    len: AtomicUsize,
}

impl SyncWaker {
    /// Creates a new `SyncWaker`.
    #[inline]
    pub fn new() -> Self {
        SyncWaker {
            operations: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    /// Registers the current thread with `select`.
    #[inline]
    pub fn register(&self, select: Select) {
        let mut operations = self.operations.lock();
        operations.push_back(Operation {
            context: context::current(),
            select,
        });
        self.len.store(operations.len(), Ordering::SeqCst);
    }

    /// Unregisters the current thread with `select`.
    #[inline]
    pub fn unregister(&self, select: Select) -> Option<Operation> {
        if self.len.load(Ordering::SeqCst) > 0 {
            let mut operations = self.operations.lock();

            if let Some((i, _)) = operations.iter()
                .enumerate()
                .find(|&(_, operation)| operation.select == select)
            {
                let operation = operations.remove(i);
                self.len.store(operations.len(), Ordering::SeqCst);
                Self::maybe_shrink(&mut operations);
                operation
            } else {
                None
            }
        } else {
            None
        }
    }

    #[inline]
    pub fn wake_one(&self) -> Option<Operation> {
        if self.len.load(Ordering::SeqCst) > 0 {
            self.wake_one_check()
        } else {
            None
        }
    }

    fn wake_one_check(&self) -> Option<Operation> {
        let thread_id = context::current_thread_id();
        let mut operations = self.operations.lock();

        for i in 0..operations.len() {
            if operations[i].context.thread_id() != thread_id {
                if operations[i].context.try_select(operations[i].select, 0).is_ok() {
                    let operation = operations.remove(i).unwrap();
                    self.len.store(operations.len(), Ordering::SeqCst);
                    Self::maybe_shrink(&mut operations);

                    drop(operations);
                    operation.context.unpark();
                    return Some(operation);
                }
            }
        }
        None
    }

    /// TODO Aborts all registered selection operations.
    pub fn close(&self) {
        // TODO: explain why not drain
        for operation in self.operations.lock().iter() {
            if operation.context.try_select(Select::Closed, 0).is_ok() {
                operation.context.unpark();
            }
        }
    }

    /// Returns `true` if there exists a operation which isn't owned by the current thread.
    #[inline]
    pub fn can_notify(&self) -> bool {
        if self.len.load(Ordering::SeqCst) > 0 {
            self.can_notify_check()
        } else {
            false
        }
    }

    fn can_notify_check(&self) -> bool {
        let operations = self.operations.lock();
        let thread_id = context::current_thread_id();

        for i in 0..operations.len() {
            if operations[i].context.thread_id() != thread_id {
                return true;
            }
        }
        false
    }

    /// Shrinks the internal deque if it's capacity is much larger than length.
    #[inline]
    fn maybe_shrink(operations: &mut VecDeque<Operation>) {
        if operations.capacity() > 32 && operations.len() < operations.capacity() / 4 {
            Self::shrink(operations);
        }
    }

    fn shrink(operations: &mut VecDeque<Operation>) {
        let mut v = VecDeque::with_capacity(operations.capacity() / 2);
        v.extend(operations.drain(..));
        *operations = v;
    }
}

impl Drop for SyncWaker {
    #[inline]
    fn drop(&mut self) {
        debug_assert!(self.operations.lock().is_empty());
        debug_assert_eq!(self.len.load(Ordering::SeqCst), 0);
    }
}
