use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;

use actor::{self, Actor};

pub struct Packet<T>(Mutex<Option<T>>);

impl<T> Packet<T> {
    pub fn new(data: Option<T>) -> Self {
        Packet(Mutex::new(data))
    }

    pub fn put(&self, data: T) {
        let mut opt = self.0.lock().unwrap();
        assert!(opt.is_none());
        *opt = Some(data);
    }

    pub fn take(&self) -> T {
        self.0.lock().unwrap().take().unwrap()
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

pub struct Dock<T>(VecDeque<Entry<T>>);

impl<T> Dock<T> {
    pub fn new() -> Self {
        Dock(VecDeque::new())
    }

    pub fn pop(&mut self) -> Option<Entry<T>> {
        self.0.pop_front()
    }

    pub fn register_offer(&mut self, packet: *const Packet<T>) {
        self.0.push_back(Entry::Offer {
            actor: actor::current(),
            packet,
        });
    }

    pub fn register_promise(&mut self) {
        self.0.push_back(Entry::Promise {
            actor: actor::current(),
        });
    }

    pub fn unregister(&mut self) {
        let id = thread::current().id();
        self.0.retain(|s| match *s {
            Entry::Promise { ref actor } => actor.thread_id() != id,
            Entry::Offer { ref actor, .. } => actor.thread_id() != id,
        });
        self.maybe_shrink();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn notify_all(&mut self) {
        for t in self.0.drain(..) {
            let actor = match t {
                Entry::Promise { actor } => actor,
                Entry::Offer { actor, .. } => actor,
            };
            if actor.select(1) {
                actor.unpark();
            }
        }
        self.maybe_shrink();
    }

    pub fn maybe_shrink(&mut self) {
        if self.0.capacity() > 32 && self.0.capacity() / 2 > self.0.len() {
            self.0.shrink_to_fit();
        }
    }
}

impl<T> Drop for Dock<T> {
    fn drop(&mut self) {
        debug_assert!(self.is_empty());
    }
}
