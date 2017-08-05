use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;

use parking_lot::Mutex;

use actor::{self, Actor};

pub struct Packet<T>(Mutex<Option<T>>);

impl<T> Packet<T> {
    pub fn new(data: Option<T>) -> Self {
        Packet(Mutex::new(data))
    }

    pub fn put(&self, data: T) {
        let mut opt = self.0.lock();
        assert!(opt.is_none());
        *opt = Some(data);
    }

    pub fn take(&self) -> T {
        self.0.lock().take().unwrap()
    }
}

pub struct Request<T> {
    pub actor: Arc<Actor>,
    pub packet: Packet<T>,
}

impl<T> Request<T> {
    pub fn new(data: Option<T>) -> Self {
        Request {
            actor: actor::current(),
            packet: Packet::new(data),
        }
    }
}

pub enum Entry<T> {
    Promise { actor: Arc<Actor> },
    Offer {
        actor: Arc<Actor>,
        packet: *const Packet<T>,
    },
}

impl<T> Entry<T> {
    pub fn is_current(&self) -> bool {
        match *self {
            Entry::Promise { ref actor } => actor.thread_id() == thread::current().id(),
            Entry::Offer { ref actor, .. } => actor.thread_id() == thread::current().id(),
        }
    }
}

pub struct Dock<T> {
    pub entries: Mutex<VecDeque<Entry<T>>>, // TODO: make it private
    len: AtomicUsize,
}

impl<T> Dock<T> {
    pub fn new() -> Self {
        Dock {
            entries: Mutex::new(VecDeque::new()),
            len: AtomicUsize::new(0),
        }
    }

    pub fn pop(&self) -> Option<Entry<T>> {
        if self.len.load(SeqCst) > 0 {
            let mut entries = self.entries.lock();

            if let Some(e) = entries.pop_front() {
                self.len.store(entries.len(), SeqCst);
                return Some(e);
            }
        }
        None
    }

    pub fn register_offer(&self, packet: *const Packet<T>) {
        let mut entries = self.entries.lock();
        entries.push_back(Entry::Offer {
            actor: actor::current(),
            packet,
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn register_promise(&self) {
        let mut entries = self.entries.lock();
        entries.push_back(Entry::Promise {
            actor: actor::current(),
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn unregister(&self) {
        let mut entries = self.entries.lock();
        let thread_id = thread::current().id();

        entries.retain(|s| match *s {
            Entry::Promise { ref actor } => actor.thread_id() != thread_id,
            Entry::Offer { ref actor, .. } => actor.thread_id() != thread_id,
        });
        // self.maybe_shrink();
        self.len.store(entries.len(), SeqCst);
    }

    pub fn is_empty(&self) -> bool {
        self.len.load(SeqCst) == 0
    }

    pub fn notify_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut entries = self.entries.lock();
            self.len.store(0, SeqCst);

            for t in entries.drain(..) {
                let actor = match t {
                    Entry::Promise { actor } => actor,
                    Entry::Offer { actor, .. } => actor,
                };
                actor.select(1);
                actor.unpark();
            }
            // self.maybe_shrink();
        }
    }

    // pub fn maybe_shrink(&mut self) {
    //     if self.0.capacity() > 32 && self.0.capacity() / 2 > self.0.len() {
    //         self.0.shrink_to_fit();
    //     }
    // }
}

impl<T> Drop for Dock<T> {
    fn drop(&mut self) {
        debug_assert!(self.is_empty());
    }
}
