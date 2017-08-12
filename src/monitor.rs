use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;

use parking_lot::Mutex;

use CaseId;
use actor::{self, Actor};

// TODO: Explain that a single thread can be registered multiple times (that happens only in
// select).  Unregister removes just entry belonging to the current thread.

struct Entry {
    actor: Arc<Actor>,
    case_id: CaseId,
}

pub(crate) struct Monitor {
    entries: Mutex<VecDeque<Entry>>,
    len: AtomicUsize,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            entries: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn register(&self, case_id: CaseId) {
        let mut entries = self.entries.lock();
        entries.push_back(Entry {
            actor: actor::current(),
            case_id,
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn unregister(&self, case_id: CaseId) {
        let thread_id = thread::current().id();
        let mut entries = self.entries.lock();

        if let Some((i, _)) = entries.iter().enumerate().find(|&(_, e)| {
            e.case_id == case_id && e.actor.thread_id() == thread_id
        }) {
            entries.remove(i);
            self.len.store(entries.len(), SeqCst);
            self.maybe_shrink(&mut entries);
        }
    }

    pub fn notify_one(&self) {
        if self.len.load(SeqCst) > 0 {
            let thread_id = thread::current().id();
            let mut entries = self.entries.lock();

            let mut i = 0;
            while i < entries.len() {
                if entries[i].actor.thread_id() != thread_id {
                    let e = entries.remove(i).unwrap();
                    self.len.store(entries.len(), SeqCst);
                    self.maybe_shrink(&mut entries);

                    if e.actor.select(e.case_id) {
                        e.actor.unpark();
                        break;
                    }
                }
                i += 1;
            }
        }
    }

    pub fn abort_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut entries = self.entries.lock();

            self.len.store(0, SeqCst);
            for e in entries.drain(..) {
                if e.actor.select(CaseId::abort()) {
                    e.actor.unpark();
                }
            }

            self.maybe_shrink(&mut entries);
        }
    }

    fn maybe_shrink(&self, entries: &mut VecDeque<Entry>) {
        if entries.capacity() > 32 && entries.capacity() / 2 > entries.len() {
            entries.shrink_to_fit();
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        debug_assert!(self.entries.lock().is_empty());
        debug_assert_eq!(self.len.load(SeqCst), 0);
    }
}
