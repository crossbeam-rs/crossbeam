use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

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
}

/// A simple wait queue for list-based and array-based channels.
///
/// This data structure is used for registering selection cases before blocking and waking them
/// up when the channel receives a message, sends one, or gets closed.
pub struct SyncWaker {
    /// The list of registered selection cases.
    cases: Mutex<VecDeque<Case>>,

    /// Number of cases in the list.
    len: AtomicUsize,
}

impl SyncWaker {
    /// Creates a new `SyncWaker`.
    #[inline]
    pub fn new() -> Self {
        SyncWaker {
            cases: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    /// Registers the current thread with `case_id`.
    #[inline]
    pub fn register(&self, case_id: CaseId) {
        let mut cases = self.cases.lock();
        cases.push_back(Case {
            context: context::current(),
            case_id,
        });
        self.len.store(cases.len(), Ordering::SeqCst);
    }

    /// Unregisters the current thread with `case_id`.
    #[inline]
    pub fn unregister(&self, case_id: CaseId) -> Option<Case> {
        if self.len.load(Ordering::SeqCst) > 0 {
            let mut cases = self.cases.lock();

            if let Some((i, _)) = cases.iter()
                .enumerate()
                .find(|&(_, case)| case.case_id == case_id)
            {
                let case = cases.remove(i);
                self.len.store(cases.len(), Ordering::SeqCst);
                Self::maybe_shrink(&mut cases);
                case
            } else {
                None
            }
        } else {
            None
        }
    }

    #[inline]
    pub fn wake_one(&self) -> Option<Case> {
        if self.len.load(Ordering::SeqCst) > 0 {
            self.wake_one_check()
        } else {
            None
        }
    }

    fn wake_one_check(&self) -> Option<Case> {
        let thread_id = context::current_thread_id();
        let mut cases = self.cases.lock();

        for i in 0..cases.len() {
            if cases[i].context.thread.id() != thread_id {
                if cases[i].context.try_select(cases[i].case_id, 0) {
                    let case = cases.remove(i).unwrap();
                    self.len.store(cases.len(), Ordering::SeqCst);
                    Self::maybe_shrink(&mut cases);

                    drop(cases);
                    case.context.unpark();
                    return Some(case);
                }
            }
        }
        None
    }

    /// TODO Aborts all registered selection cases.
    pub fn close(&self) {
        // TODO: explain why not drain
        for case in self.cases.lock().iter() {
            if case.context.try_select(CaseId::Closed, 0) {
                case.context.unpark();
            }
        }
    }

    /// Returns `true` if there exists a case which isn't owned by the current thread.
    #[inline]
    pub fn can_notify(&self) -> bool {
        if self.len.load(Ordering::SeqCst) > 0 {
            self.can_notify_check()
        } else {
            false
        }
    }

    fn can_notify_check(&self) -> bool {
        let cases = self.cases.lock();
        let thread_id = context::current_thread_id();

        for i in 0..cases.len() {
            if cases[i].context.thread.id() != thread_id {
                return true;
            }
        }
        false
    }

    /// Shrinks the internal deque if it's capacity is much larger than length.
    #[inline]
    fn maybe_shrink(cases: &mut VecDeque<Case>) {
        if cases.capacity() > 32 && cases.len() < cases.capacity() / 4 {
            Self::shrink(cases);
        }
    }

    fn shrink(cases: &mut VecDeque<Case>) {
        let mut v = VecDeque::with_capacity(cases.capacity() / 2);
        v.extend(cases.drain(..));
        *cases = v;
    }
}

impl Drop for SyncWaker {
    #[inline]
    fn drop(&mut self) {
        debug_assert!(self.cases.lock().is_empty());
        debug_assert_eq!(self.len.load(Ordering::SeqCst), 0);
    }
}
