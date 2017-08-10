use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use parking_lot::Mutex;

// TODO: this is actually selection id. Should it be called SelectId, or maybe Case?
// TODO: Note that this is non-zero
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HandleId(usize);

impl HandleId {
    pub fn new(num: usize) -> Self {
        HandleId(num)
    }

    pub fn sentinel() -> Self {
        HandleId(1)
    }
}

pub struct Actor {
    select_id: AtomicUsize,
    request_ptr: AtomicUsize,
    thread: Thread,
}

impl Actor {
    pub fn select(&self, id: HandleId) -> bool {
        self.select_id.compare_and_swap(0, id.0, SeqCst) == 0
    }

    pub fn unpark(&self) {
        self.thread.unpark();
    }

    // TODO: maybe this needs to be unsafe and {put,take}_request safe?
    pub fn set_request<T>(&self, req: *const Request<T>) {
        self.request_ptr.store(req as usize, SeqCst);
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread.id()
    }

    pub fn reset(&self) {
        self.select_id.store(0, SeqCst);
        self.request_ptr.store(0, SeqCst);
    }

    pub fn selected(&self) -> HandleId {
        HandleId::new(self.select_id.load(SeqCst))
    }

    pub fn wait_until(&self, deadline: Option<Instant>) -> bool {
        for i in 0..10 {
            if self.select_id.load(SeqCst) != 0 {
                return true;
            }
            for _ in 0 .. 1 << i {
                // ::std::sync::atomic::hint_core_should_pause();
            }
        }

        for _ in 0..10 {
            if self.select_id.load(SeqCst) != 0 {
                return true;
            }
            thread::yield_now();
        }

        while self.select_id.load(SeqCst) == 0 {
            let now = Instant::now();

            if let Some(end) = deadline {
                if now < end {
                    thread::park_timeout(end - now);
                } else if self.select(HandleId::sentinel()) {
                    return false;
                }
            } else {
                thread::park();
            }
        }

        true
    }

    pub unsafe fn put_request<T>(&self, value: T, id: HandleId) {
        let req = loop {
            let ptr = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
            if !ptr.is_null() {
                break ptr;
            }
            thread::yield_now();
        };

        let thread = (*req).actor.thread.clone();
        (*req).packet.put(value);
        (*req).actor.select(id);
        thread.unpark();
    }

    pub unsafe fn take_request<T>(&self, id: HandleId) -> T {
        let req = loop {
            let ptr = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
            if !ptr.is_null() {
                break ptr;
            }
            thread::yield_now();
        };

        let thread = (*req).actor.thread.clone();
        let v = (*req).packet.take().unwrap();
        (*req).actor.select(id);
        thread.unpark();
        v
    }
}

thread_local! {
    static ACTOR: Arc<Actor> = Arc::new(Actor {
        select_id: AtomicUsize::new(0),
        request_ptr: AtomicUsize::new(0),
        thread: thread::current(),
    });
}

pub fn current() -> Arc<Actor> {
    ACTOR.with(|a| a.clone())
}

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
            actor: current(),
            packet: Packet::new(data),
        }
    }
}
