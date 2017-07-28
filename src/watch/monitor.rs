use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};

use actor::{self, Actor};

pub struct Monitor {
    actors: Mutex<VecDeque<Arc<Actor>>>,
    len: AtomicUsize,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            actors: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn register(&self) {
        let mut actors = self.actors.lock().unwrap();
        actors.push_back(actor::current());
        self.len.store(actors.len(), SeqCst);
    }

    pub fn unregister(&self) {
        let id = thread::current().id();
        let mut actors = self.actors.lock().unwrap();

        if let Some((i, _)) = actors
            .iter()
            .enumerate()
            .find(|&(_, a)| a.thread_id() == id)
        {
            actors.remove(i);
            self.len.store(actors.len(), SeqCst);
        }
    }

    pub fn notify_one(&self, id: usize) {
        if self.len.load(SeqCst) > 0 {
            let mut actors = self.actors.lock().unwrap();

            while let Some(a) = actors.pop_front() {
                self.len.store(actors.len(), SeqCst);

                if a.select(id) {
                    a.unpark();
                    break;
                }
            }
        }
    }

    pub fn notify_all(&self, id: usize) {
        if self.len.load(SeqCst) > 0 {
            let mut actors = self.actors.lock().unwrap();

            while let Some(a) = actors.pop_front() {
                self.len.store(actors.len(), SeqCst);

                if a.select(id) {
                    a.unpark();
                }
            }
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if cfg!(debug_assertions) {
            let actors = self.actors.lock().unwrap();
            debug_assert_eq!(actors.len(), 0);
            debug_assert_eq!(self.len.load(SeqCst), 0);
        }
    }
}
