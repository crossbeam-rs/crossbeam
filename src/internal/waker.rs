use std::collections::VecDeque;
use std::sync::Arc;

use internal::context::{self, Context};
use internal::select::CaseId;

/// A selection case, identified by a `Context` and a `CaseId`.
///
/// Note that multiple threads could be operating on a single channel end, as well as a single
/// thread on multiple different channel ends.
pub struct Case {
    /// A context associated with the thread owning this case.
    pub context: Arc<Context>,

    /// The case ID.
    pub case_id: CaseId,

    pub packet: usize,
}

/// A simple wait queue for zero-capacity channels.
///
/// This data structure is used for registering selection cases before blocking and waking them
/// up when the channel receives a message, sends one, or gets closed.
pub struct Waker {
    /// The list of registered selection cases.
    cases: VecDeque<Case>,
}

impl Waker {
    /// Creates a new `Waker`.
    #[inline]
    pub fn new() -> Self {
        Waker {
            cases: VecDeque::new(),
        }
    }

    #[inline]
    pub fn register_with_packet(&mut self, case_id: CaseId, packet: usize) {
        self.cases.push_back(Case {
            context: context::current(),
            case_id,
            packet,
        });
    }

    /// Unregisters the current thread with `case_id`.
    #[inline]
    pub fn unregister(&mut self, case_id: CaseId) -> Option<Case> {
        if let Some((i, _)) = self.cases
            .iter()
            .enumerate()
            .find(|&(_, case)| case.case_id == case_id)
        {
            let case = self.cases.remove(i);
            Self::maybe_shrink(&mut self.cases);
            case
        } else {
            None
        }
    }

    #[inline]
    pub fn wake_one(&mut self) -> Option<Case> {
        if !self.cases.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.cases.len() {
                if self.cases[i].context.thread.id() != thread_id {
                    if self.cases[i].context.try_select(
                        self.cases[i].case_id,
                        self.cases[i].packet,
                    ) {
                        let case = self.cases.remove(i).unwrap();
                        Self::maybe_shrink(&mut self.cases);

                        case.context.unpark();
                        return Some(case);
                    }
                }
            }
        }

        None
    }

    /// TODO Aborts all registered selection cases.
    #[inline]
    pub fn close(&mut self) {
        // TODO: explain why not drain
        for case in self.cases.iter() {
            if case.context.try_select(CaseId::Closed, 0) {
                case.context.unpark();
            }
        }
    }

    /// Returns `true` if there exists a case which isn't owned by the current thread.
    #[inline]
    // TODO: rename contains_other_threads?
    pub fn can_notify(&self) -> bool {
        if !self.cases.is_empty() {
            let thread_id = context::current_thread_id();

            for i in 0..self.cases.len() {
                if self.cases[i].context.thread.id() != thread_id {
                    return true;
                }
            }
        }
        false
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    /// Shrinks the internal deque if it's capacity is much larger than length.
    #[inline]
    fn maybe_shrink(cases: &mut VecDeque<Case>) {
        if cases.capacity() > 32 && cases.len() < cases.capacity() / 4 {
            let mut v = VecDeque::with_capacity(cases.capacity() / 2);
            v.extend(cases.drain(..));
            *cases = v;
        }
    }
}

impl Drop for Waker {
    #[inline]
    fn drop(&mut self) {
        debug_assert!(self.cases.is_empty());
    }
}
