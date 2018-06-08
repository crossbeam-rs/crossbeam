use std::collections::VecDeque;
use std::sync::Arc;

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

    pub packet: usize,
}

/// A simple wait queue for zero-capacity channels.
///
/// This data structure is used for registering selection operations before blocking and waking
/// them up when the channel receives a message, sends one, or gets closed.
pub struct Waker {
    /// The list of registered selection operations.
    operations: VecDeque<Operation>,
}

impl Waker {
    /// Creates a new `Waker`.
    #[inline]
    pub fn new() -> Self {
        Waker {
            operations: VecDeque::new(),
        }
    }

    #[inline]
    pub fn register_with_packet(&mut self, select: Select, packet: usize) {
        self.operations.push_back(Operation {
            context: context::current(),
            select,
            packet,
        });
    }

    /// Unregisters the current thread with `select`.
    #[inline]
    pub fn unregister(&mut self, select: Select) -> Option<Operation> {
        if let Some((i, _)) = self.operations
            .iter()
            .enumerate()
            .find(|&(_, operation)| operation.select == select)
        {
            let operation = self.operations.remove(i);
            Self::maybe_shrink(&mut self.operations);
            operation
        } else {
            None
        }
    }

    #[inline]
    pub fn wake_one(&mut self) -> Option<Operation> {
        if !self.operations.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.operations.len() {
                if self.operations[i].context.thread_id != thread_id {
                    if self.operations[i].context.try_select(
                        self.operations[i].select,
                        self.operations[i].packet,
                    ) {
                        let operation = self.operations.remove(i).unwrap();
                        Self::maybe_shrink(&mut self.operations);

                        operation.context.unpark();
                        return Some(operation);
                    }
                }
            }
        }

        None
    }

    /// TODO Aborts all registered selection operations.
    #[inline]
    pub fn close(&mut self) {
        // TODO: explain why not drain
        for operation in self.operations.iter() {
            if operation.context.try_select(Select::Closed, 0) {
                operation.context.unpark();
            }
        }
    }

    /// Returns `true` if there exists a operation which isn't owned by the current thread.
    #[inline]
    // TODO: rename contains_other_threads?
    pub fn can_notify(&self) -> bool {
        if !self.operations.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.operations.len() {
                if self.operations[i].context.thread_id != thread_id {
                    return true;
                }
            }
        }
        false
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Shrinks the internal deque if it's capacity is much larger than length.
    #[inline]
    fn maybe_shrink(operations: &mut VecDeque<Operation>) {
        if operations.capacity() > 32 && operations.len() < operations.capacity() / 4 {
            let mut v = VecDeque::with_capacity(operations.capacity() / 2);
            v.extend(operations.drain(..));
            *operations = v;
        }
    }
}

impl Drop for Waker {
    #[inline]
    fn drop(&mut self) {
        debug_assert!(self.operations.is_empty());
    }
}
