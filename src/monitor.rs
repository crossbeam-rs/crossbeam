use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};

pub struct Monitor {
    threads: Mutex<VecDeque<Thread>>,
    len: AtomicUsize,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            threads: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn watch_start(&self) -> bool {
        let mut threads = self.threads.lock().unwrap();
        let id = thread::current().id();

        if threads.iter().all(|t| t.id() != id) {
            threads.push_back(thread::current());
            self.len.store(threads.len(), SeqCst);
            true
        } else {
            false
        }
    }

    pub fn watch_stop(&self) -> bool {
        let mut threads = self.threads.lock().unwrap();
        let id = thread::current().id();

        if let Some((i, _)) = threads.iter().enumerate().find(|&(_, t)| t.id() == id) {
            threads.remove(i);
            self.len.store(threads.len(), SeqCst);
            true
        } else {
            false
        }
    }

    pub fn watch_abort(&self) -> bool {
        let mut threads = self.threads.lock().unwrap();
        let id = thread::current().id();

        if let Some((i, _)) = threads.iter().enumerate().find(|&(_, t)| t.id() == id) {
            threads.remove(i);
            self.len.store(threads.len(), SeqCst);
            return true;
        }

        if let Some(t) = threads.pop_front() {
            self.len.store(threads.len(), SeqCst);
            t.unpark();
        }
        false
    }

    pub fn notify_one(&self) -> bool {
        if self.len.load(SeqCst) > 0 {
            let mut threads = self.threads.lock().unwrap();

            if let Some(t) = threads.pop_front() {
                self.len.store(threads.len(), SeqCst);
                t.unpark();
                return true;
            }
        }
        false
    }

    pub fn notify_all(&self) -> bool {
        if self.len.load(SeqCst) > 0 {
            let mut threads = self.threads.lock().unwrap();

            if !threads.is_empty() {
                self.len.store(0, SeqCst);
                for t in threads.drain(..) {
                    t.unpark();
                }
                return true;
            }
        }
        false
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if cfg!(debug_assertions) {
            let threads = self.threads.lock().unwrap();
            debug_assert_eq!(threads.len(), 0);
            debug_assert_eq!(self.len.load(SeqCst), 0);
        }
    }
}
