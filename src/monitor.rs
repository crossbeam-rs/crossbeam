use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;

use parking_lot::Mutex;

use select::CaseId;
use select::handle::{self, Handle};

// TODO: Explain that a single thread can be registered multiple times (that happens only in
// select).  Unregister removes just entry belonging to the current thread.

struct Case {
    handle: Handle,
    case_id: CaseId,
}

/// A simple data structure that registers selection cases and notifies threads.
pub struct Monitor {
    /// The list of registered selection cases.
    cases: Mutex<VecDeque<Case>>,
    /// Number of cases in the list.
    len: AtomicUsize,
}

impl Monitor {
    /// Creates a new `Monitor`.
    pub fn new() -> Self {
        Monitor {
            cases: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    /// Registers the current thread with given `case_id`.
    pub fn register(&self, case_id: CaseId) {
        let mut cases = self.cases.lock();
        cases.push_back(Case {
            handle: handle::current(),
            case_id,
        });
        self.len.store(cases.len(), SeqCst);
    }

    /// Unregisters the current thread with given `case_id`.
    pub fn unregister(&self, case_id: CaseId) {
        let thread_id = thread::current().id();
        let mut cases = self.cases.lock();

        if let Some((i, _)) = cases.iter().enumerate().find(|&(_, case)| {
            case.case_id == case_id && case.handle.thread_id() == thread_id
        }) {
            cases.remove(i);
            self.len.store(cases.len(), SeqCst);
            self.maybe_shrink(&mut cases);
        }
    }

    /// Fires one selection from another thread.
    pub fn notify_one(&self) {
        if self.len.load(SeqCst) > 0 {
            let thread_id = thread::current().id();
            let mut cases = self.cases.lock();

            let mut i = 0;
            while i < cases.len() {
                if cases[i].handle.thread_id() != thread_id {
                    let case = cases.remove(i).unwrap();
                    self.len.store(cases.len(), SeqCst);
                    self.maybe_shrink(&mut cases);

                    if case.handle.select(case.case_id) {
                        case.handle.unpark();
                        break;
                    }
                }
                i += 1;
            }
        }
    }

    /// Aborts all currently registered selection cases.
    pub fn abort_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut cases = self.cases.lock();

            self.len.store(0, SeqCst);
            for case in cases.drain(..) {
                if case.handle.select(CaseId::abort()) {
                    case.handle.unpark();
                }
            }

            self.maybe_shrink(&mut cases);
        }
    }

    fn maybe_shrink(&self, cases: &mut VecDeque<Case>) {
        if cases.capacity() > 32 && cases.len() < cases.capacity() / 2 {
            cases.shrink_to_fit();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        debug_assert!(self.cases.lock().is_empty());
        debug_assert_eq!(self.len.load(SeqCst), 0);
    }
}
