use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;

use parking_lot::Mutex;

use select::CaseId;
use select::handle::{self, Handle};

/// A selection case, identified by a `Handle` and a `CaseId`.
///
/// Note that multiple threads could be operating on a single channel end, as well as a single
/// thread on multiple different channel ends.
struct Case {
    /// A handle associated with the thread owning this case.
    handle: Handle,

    /// The case ID.
    case_id: CaseId,
}

/// A simple wait queue for list-based and array-based channels.
///
/// This data structure is used sfor registeroing selection cases before blocking and waking them
/// up when the channel receives a message, sends one, or gets closed.
pub struct Monitor {
    /// The list of registered selection cases.
    cases: Mutex<VecDeque<Case>>,

    /// Number of cases in the list.
    len: AtomicUsize,
}

impl Monitor {
    /// Creates a new `Monitor`.
    #[inline]
    pub fn new() -> Self {
        Monitor {
            cases: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    /// Registers the current thread with `case_id`.
    pub fn register(&self, case_id: CaseId) {
        let mut cases = self.cases.lock();
        cases.push_back(Case {
            handle: handle::current(),
            case_id,
        });
        self.len.store(cases.len(), SeqCst);
    }

    /// Unregisters the current thread with `case_id`.
    pub fn unregister(&self, case_id: CaseId) {
        let thread_id = thread::current().id();
        let mut cases = self.cases.lock();

        if let Some((i, _)) = cases.iter().enumerate().find(|&(_, case)| {
            case.case_id == case_id && case.handle.thread_id() == thread_id
        }) {
            cases.remove(i);
            self.len.store(cases.len(), SeqCst);
            Self::maybe_shrink(&mut cases);
        }
    }

    /// Attempts to fire one case which is owned by another thread.
    pub fn notify_one(&self) {
        if self.len.load(SeqCst) > 0 {
            let thread_id = thread::current().id();
            let mut cases = self.cases.lock();

            for i in 0..cases.len() {
                if cases[i].handle.thread_id() != thread_id {
                    if cases[i].handle.try_select(cases[i].case_id) {
                        let case = cases.remove(i).unwrap();
                        self.len.store(cases.len(), SeqCst);
                        Self::maybe_shrink(&mut cases);

                        case.handle.unpark();
                        break;
                    }
                }
            }
        }
    }

    /// Aborts all currently registered selection cases.
    pub fn abort_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut cases = self.cases.lock();

            self.len.store(0, SeqCst);
            for case in cases.drain(..) {
                if case.handle.try_select(CaseId::abort()) {
                    case.handle.unpark();
                }
            }

            Self::maybe_shrink(&mut cases);
        }
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

impl Drop for Monitor {
    fn drop(&mut self) {
        debug_assert!(self.cases.lock().is_empty());
        debug_assert_eq!(self.len.load(SeqCst), 0);
    }
}
