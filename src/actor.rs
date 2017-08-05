use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use Backoff;
use watch::dock::Request;

// TODO: hide all pub fields
// TODO: type safe QueueId

pub struct Actor {
    select_id: AtomicUsize,
    request_ptr: AtomicUsize,
    thread: Thread,
}

impl Actor {
    pub fn select(&self, id: usize) -> bool {
        self.select_id.compare_and_swap(0, id, SeqCst) == 0
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

    pub fn selected(&self) -> usize {
        self.select_id.load(SeqCst)
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
                } else if self.select(1) {
                    return false;
                }
            } else {
                thread::park();
            }
        }

        true
    }

    pub unsafe fn put_request<T>(&self, value: T, id: usize) {
        let req = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
        assert!(!req.is_null());

        unsafe {
            let thread = (*req).actor.thread.clone();
            (*req).packet.put(value);
            (*req).actor.select(id);
            thread.unpark();
        }
    }

    pub unsafe fn take_request<T>(&self, id: usize) -> T {
        let req = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
        assert!(!req.is_null());

        unsafe {
            let thread = (*req).actor.thread.clone();
            let v = (*req).packet.take();
            (*req).actor.select(id);
            thread.unpark();
            v
        }
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
