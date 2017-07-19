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

    pub fn subscribe(&self) {
        let mut threads = self.threads.lock().unwrap();
        threads.push_back(thread::current());
        self.len.store(threads.len(), SeqCst);
    }

    pub fn unsubscribe(&self) {
        let mut threads = self.threads.lock().unwrap();
        let id = thread::current().id();

        if let Some((i, _)) = threads.iter().enumerate().find(|&(_, t)| t.id() == id) {
            threads.remove(i);
            self.len.store(threads.len(), SeqCst);
        } else if let Some(t) = threads.pop_front() {
            self.len.store(threads.len(), SeqCst);
            t.unpark();
        }
    }

    pub fn notify_one(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut threads = self.threads.lock().unwrap();

            if let Some(t) = threads.pop_front() {
                self.len.store(threads.len(), SeqCst);
                t.unpark();
            }
        }
    }

    pub fn notify_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut threads = self.threads.lock().unwrap();

            self.len.store(0, SeqCst);
            for t in threads.drain(..) {
                t.unpark();
            }
        }
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
