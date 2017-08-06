use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use parking_lot::Mutex;

use Backoff;

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
        let mut backoff = Backoff::new();
        while self.select_id.load(SeqCst) == 0 {
            if !backoff.tick() {
                break;
            }
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
        let req = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
        assert!(!req.is_null());

        let thread = (*req).actor.thread.clone();
        (*req).packet.put(value);
        (*req).actor.select(id);
        thread.unpark();
    }

    pub unsafe fn take_request<T>(&self, id: HandleId) -> T {
        let req = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
        assert!(!req.is_null());

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
