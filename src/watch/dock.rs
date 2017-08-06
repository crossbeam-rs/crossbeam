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

    pub fn take(&self) -> Option<T> {
        self.0.lock().take()
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

// TODO: refactor this, it's awkward
pub enum Entry<T> {
    Promise { actor: Arc<Actor>, id: usize },
    Offer {
        actor: Arc<Actor>,
        id: usize,
        packet: *const Packet<T>,
    },
}

impl<T> Entry<T> {
    pub fn is_current(&self) -> bool {
        match *self {
            Entry::Promise { ref actor, .. } => actor.thread_id() == thread::current().id(),
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
            let thread_id = thread::current().id();
            let mut entries = self.entries.lock();

            for i in 0..entries.len() {
                let tid = match entries[i] {
                    Entry::Promise { ref actor, .. } => actor.thread_id(),
                    Entry::Offer { ref actor, .. } => actor.thread_id(),
                };
                if tid != thread_id {
                    let e = entries.remove(i).unwrap();
                    self.len.store(entries.len(), SeqCst);
                    return Some(e);
                }
            }
        }
        None
    }

    pub fn register_offer(&self, packet: *const Packet<T>, id: usize) {
        let mut entries = self.entries.lock();
        entries.push_back(Entry::Offer {
            actor: actor::current(),
            id,
            packet,
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn register_promise(&self, id: usize) {
        let mut entries = self.entries.lock();
        entries.push_back(Entry::Promise {
            actor: actor::current(),
            id,
        });
        self.len.store(entries.len(), SeqCst);
    }

    pub fn unregister(&self, my_id: usize) {
        let thread_id = thread::current().id();
        let mut entries = self.entries.lock();

        entries.retain(|s| match *s {
            Entry::Promise { ref actor, id } => actor.thread_id() != thread_id || id != my_id,
            Entry::Offer { ref actor, id, .. } => actor.thread_id() != thread_id || id != my_id,
        });
        // self.maybe_shrink();
        self.len.store(entries.len(), SeqCst);
    }

    pub fn is_empty(&self) -> bool {
        self.len.load(SeqCst) == 0
    }

    pub fn can_notify(&self) -> bool {
        if self.len.load(SeqCst) > 0 {
            let thread_id = thread::current().id();
            let mut entries = self.entries.lock();

            for i in 0..entries.len() {
                let tid = match entries[i] {
                    Entry::Promise { ref actor, .. } => actor.thread_id(),
                    Entry::Offer { ref actor, .. } => actor.thread_id(),
                };
                if tid != thread_id {
                    return true;
                }
            }
        }
        false
    }

    pub fn notify_all(&self) {
        if self.len.load(SeqCst) > 0 {
            let mut entries = self.entries.lock();
            self.len.store(0, SeqCst);

            // TODO: should we ignore current thread? it's probably fine
            for t in entries.drain(..) {
                let (actor, id) = match t {
                    Entry::Promise { actor, id } => (actor, id),
                    Entry::Offer { actor, id, .. } => (actor, id),
                };
                actor.select(id);
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
