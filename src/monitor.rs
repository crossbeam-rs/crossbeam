use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};

use actor::{ACTOR, Actor};

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
        ACTOR.with(|a| actors.push_back(a.clone()));
        self.len.store(actors.len(), SeqCst);
    }

    pub fn unregister(&self) {
        let mut actors = self.actors.lock().unwrap();

        ACTOR.with(|a| {
            let id = thread::current().id();

            if let Some((i, _)) = actors
                .iter()
                .enumerate()
                .find(|&(_, a)| a.thread.id() == id)
            {
                actors.remove(i);
                self.len.store(actors.len(), SeqCst);
            }
        });
    }

    pub fn wakeup_one(&self, id: usize) {
        let mut actors = self.actors.lock().unwrap();

        while let Some(a) = actors.pop_front() {
            self.len.store(actors.len(), SeqCst);

            if a.select_id.compare_and_swap(0, id, SeqCst) == 0 {
                a.thread.unpark();
                break;
            }
        }
    }

    pub fn wakeup_all(&self, id: usize) {
        let mut actors = self.actors.lock().unwrap();

        while let Some(a) = actors.pop_front() {
            self.len.store(actors.len(), SeqCst);

            if a.select_id.compare_and_swap(0, id, SeqCst) == 0 {
                a.thread.unpark();
            }
        }
    }

    // pub fn watch_start(&self) -> bool {
    //     let mut threads = self.threads.lock().unwrap();
    //     let id = thread::current().id();
    //
    //     if threads.iter().all(|t| t.id() != id) {
    //         threads.push_back(thread::current());
    //         self.len.store(threads.len(), SeqCst);
    //         true
    //     } else {
    //         false
    //     }
    // }
    //
    // pub fn watch_stop(&self) -> bool {
    //     let mut threads = self.threads.lock().unwrap();
    //     let id = thread::current().id();
    //
    //     if let Some((i, _)) = threads.iter().enumerate().find(|&(_, t)| t.id() == id) {
    //         threads.remove(i);
    //         self.len.store(threads.len(), SeqCst);
    //         true
    //     } else {
    //         false
    //     }
    // }
    //
    // pub fn watch_abort(&self) -> bool {
    //     let mut threads = self.threads.lock().unwrap();
    //     let id = thread::current().id();
    //
    //     if let Some((i, _)) = threads.iter().enumerate().find(|&(_, t)| t.id() == id) {
    //         threads.remove(i);
    //         self.len.store(threads.len(), SeqCst);
    //         return true;
    //     }
    //
    //     if let Some(t) = threads.pop_front() {
    //         self.len.store(threads.len(), SeqCst);
    //         t.unpark();
    //     }
    //     false
    // }
    //
    // pub fn notify_one(&self) -> bool {
    //     if self.len.load(SeqCst) > 0 {
    //         let mut threads = self.threads.lock().unwrap();
    //
    //         if let Some(t) = threads.pop_front() {
    //             self.len.store(threads.len(), SeqCst);
    //             t.unpark();
    //             return true;
    //         }
    //     }
    //     false
    // }
    //
    // pub fn notify_all(&self) -> bool {
    //     if self.len.load(SeqCst) > 0 {
    //         let mut threads = self.threads.lock().unwrap();
    //
    //         if !threads.is_empty() {
    //             self.len.store(0, SeqCst);
    //             for t in threads.drain(..) {
    //                 t.unpark();
    //             }
    //             return true;
    //         }
    //     }
    //     false
    // }
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
