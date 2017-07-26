use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};

use Participant;
use PARTICIPANT;

pub struct Monitor {
    // threads: Mutex<VecDeque<Thread>>,
    participants: Mutex<VecDeque<Arc<Participant>>>,
    len: AtomicUsize,
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            // threads: Mutex::new(VecDeque::new()),
            participants: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn register(&self) {
        let mut participants = self.participants.lock().unwrap();
        PARTICIPANT.with(|p| participants.push_back(p.clone()));
        self.len.store(participants.len(), SeqCst);
    }

    pub fn unregister(&self) {
        let mut participants = self.participants.lock().unwrap();

        PARTICIPANT.with(|p| {
            let id = thread::current().id();

            if let Some((i, _)) = participants
                .iter()
                .enumerate()
                .find(|&(_, p)| p.thread.id() == id)
            {
                participants.remove(i);
                self.len.store(participants.len(), SeqCst);
            }
        });
    }

    pub fn wakeup_one(&self, id: usize) {
        let mut participants = self.participants.lock().unwrap();

        while let Some(p) = participants.pop_front() {
            self.len.store(participants.len(), SeqCst);

            if p.sel.compare_and_swap(0, id, SeqCst) == 0 {
                p.thread.unpark();
                break;
            }
        }
    }

    pub fn wakeup_all(&self, id: usize) {
        let mut participants = self.participants.lock().unwrap();

        while let Some(p) = participants.pop_front() {
            self.len.store(participants.len(), SeqCst);

            if p.sel.compare_and_swap(0, id, SeqCst) == 0 {
                p.thread.unpark();
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
            let participants = self.participants.lock().unwrap();
            debug_assert_eq!(participants.len(), 0);
            debug_assert_eq!(self.len.load(SeqCst), 0);
        }
    }
}
