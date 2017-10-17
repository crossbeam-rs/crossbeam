use std::collections::VecDeque;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;

use parking_lot::Mutex;

use select::CaseId;
use select::handle::{self, Handle};

// TODO: Explain that a single thread can be registered multiple times (that happens only in
// select).  Unregister removes just entry belonging to the current thread.

struct Entry {
    handle: Handle,
    case_id: CaseId,
}

pub struct Monitor {
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
            handle: handle::current(),
            case_id,
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn unregister(&self, case_id: CaseId) {
        let thread_id = thread::current().id();
        let mut entries = self.entries.lock();

        if let Some((i, _)) = entries.iter().enumerate().find(|&(_, e)| {
            e.case_id == case_id && e.handle.thread_id() == thread_id
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
                if entries[i].handle.thread_id() != thread_id {
                    let e = entries.remove(i).unwrap();
                    self.len.store(entries.len(), SeqCst);
                    self.maybe_shrink(&mut entries);

                    if e.handle.select(e.case_id) {
                        e.handle.unpark();
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
                if e.handle.select(CaseId::abort()) {
                    e.handle.unpark();
                }
            }

            self.maybe_shrink(&mut entries);
        }
    }

    fn maybe_shrink(&self, entries: &mut VecDeque<Entry>) {
        if entries.capacity() > 32 && entries.len() < entries.capacity() / 2 {
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
