use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use parking_lot::Mutex;

use select::CaseId;
use select::handle::{self, Handle};

// TODO: Optimize current thread id

/// A selection case, identified by a `Handle` and a `CaseId`.
///
/// Note that multiple threads could be operating on a single channel end, as well as a single
/// thread on multiple different channel ends.
pub struct Case {
    /// A handle associated with the thread owning this case.
    pub handle: Arc<Handle>,

    /// The case ID.
    pub case_id: CaseId,

    pub is_prepared: bool,
}

/// A simple wait queue for list-based and array-based channels.
///
/// This data structure is used for registering selection cases before blocking and waking them
/// up when the channel receives a message, sends one, or gets closed.
pub struct Waker {
    /// The list of registered selection cases.
    cases: Mutex<VecDeque<Case>>,

    /// Number of cases in the list.
    len: AtomicUsize,
}

impl Waker {
    /// Creates a new `Waker`.
    #[inline]
    pub fn new() -> Self {
        Waker {
            cases: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    /// Registers the current thread with `case_id`.
    pub fn register(&self, case_id: CaseId, is_prepared: bool) {
        let mut cases = self.cases.lock();
        cases.push_back(Case {
            handle: handle::current(),
            case_id,
            is_prepared,
        });
        self.len.store(cases.len(), Ordering::SeqCst);
    }

    /// Unregisters the current thread with `case_id`.
    pub fn unregister(&self, case_id: CaseId) {
        let mut cases = self.cases.lock();

        if let Some((i, _)) = cases.iter().enumerate().find(|&(_, case)| case.case_id == case_id) {
            cases.remove(i);
            self.len.store(cases.len(), Ordering::SeqCst);
            Self::maybe_shrink(&mut cases);
        }
    }

    // TODO: this is an expensive call because it's not inlined?
    pub fn remove_one(&self) -> Option<Case> {
        if self.len.load(Ordering::SeqCst) > 0 {
            let thread_id = thread::current().id(); // TODO: optimize? use selection_id instead?
            let mut cases = self.cases.lock();

            for i in 0..cases.len() {
                if cases[i].handle.thread.id() != thread_id {
                    if cases[i].handle.try_select(cases[i].case_id) {
                        let case = cases.remove(i).unwrap();
                        self.len.store(cases.len(), Ordering::SeqCst);
                        Self::maybe_shrink(&mut cases);
                        return Some(case);
                    }
                }
            }
        }

        None
    }

    #[inline]
    pub fn wake_one(&self) {
        if self.len.load(Ordering::SeqCst) > 0 {
            if let Some(case) = self.remove_one() {
                case.handle.unpark();
            }
        }
    }

    /// Aborts all currently registered selection cases.
    pub fn abort_all(&self) {
        if self.len.load(Ordering::SeqCst) > 0 {
            let mut cases = self.cases.lock();

            self.len.store(0, Ordering::SeqCst);
            for case in cases.drain(..) {
                if case.handle.try_select(CaseId::abort()) {
                    case.handle.unpark();
                }
            }

            Self::maybe_shrink(&mut cases);
        }
    }

    /// Returns `true` if there exists a case which isn't owned by the current thread.
    #[inline]
    pub fn can_notify(&self) -> bool {
        if self.len.load(Ordering::SeqCst) > 0 {
            let cases = self.cases.lock();
            let thread_id = thread::current().id();

            for i in 0..cases.len() {
                if cases[i].handle.thread.id() != thread_id {
                    return true;
                }
            }
        }
        false
    }

    /// Shrinks the internal deque if it's capacity is much larger than length.
    fn maybe_shrink(cases: &mut VecDeque<Case>) {
        if cases.capacity() > 32 && cases.len() < cases.capacity() / 4 {
            let mut v = VecDeque::with_capacity(cases.capacity() / 2);
            v.extend(cases.drain(..));
            *cases = v;
        }
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        debug_assert!(self.cases.lock().is_empty());
        debug_assert_eq!(self.len.load(Ordering::SeqCst), 0);
    }
}
