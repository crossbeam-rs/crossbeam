use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;

use parking_lot::Mutex;

use actor::{self, Actor, HandleId};

// TODO: Explain that a single thread can be registered multiple times (that happens only in
// select).  Unregister removes just entry belonging to the current thread.

struct Entry {
    actor: Arc<Actor>,
    id: HandleId,
}

pub struct Monitor {
    entries: Mutex<VecDeque<Entry>>, // TODO: shrink
    len: AtomicUsize,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            entries: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn register(&self, id: HandleId) {
        let mut entries = self.entries.lock();
        entries.push_back(Entry {
            actor: actor::current(),
            id,
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn unregister(&self, id: HandleId) {
        let thread_id = thread::current().id();
        let mut entries = self.entries.lock();

        if let Some((i, _)) = entries
            .iter()
            .enumerate()
            .find(|&(_, e)| e.actor.thread_id() == thread_id && e.id == id)
        {
            entries.remove(i);
            self.len.store(entries.len(), SeqCst);
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

                    if e.actor.select(e.id) {
                        e.actor.unpark();
                        break;
                    }
                }
                i += 1;
            }
        }
    }

    pub fn notify_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let thread_id = thread::current().id();
            let mut entries = self.entries.lock();

            self.len.store(0, SeqCst);
            for e in entries.drain(..) {
                if e.actor.select(e.id) {
                    e.actor.unpark();
                }
            }
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        debug_assert!(self.entries.lock().is_empty());
        debug_assert_eq!(self.len.load(SeqCst), 0);
    }
}
