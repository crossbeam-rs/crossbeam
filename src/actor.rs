use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub struct Request<T> {
    pub actor: Arc<Actor>,
    pub data: UnsafeCell<Option<T>>,
}

impl<T> Request<T> {
    pub fn new(data: Option<T>) -> Self {
        Request {
            actor: current(),
            data: UnsafeCell::new(data),
        }
    }
}

pub struct Actor {
    pub select_id: AtomicUsize,
    pub request_ptr: AtomicUsize,
    pub thread: Thread,
}

thread_local! {
    pub static ACTOR: Arc<Actor> = Arc::new(Actor {
        select_id: AtomicUsize::new(0),
        request_ptr: AtomicUsize::new(0),
        thread: thread::current(),
    });
}

pub fn current() -> Arc<Actor> {
    ACTOR.with(|a| a.clone())
}

pub fn reset() {
    ACTOR.with(|a| a.select_id.store(0, SeqCst));
}

pub fn wait() {
    while ACTOR.with(|a| a.select_id.load(SeqCst)) == 0 {
        thread::park();
    }
}

pub fn wait_until(deadline: Option<Instant>) -> bool {
    while ACTOR.with(|a| a.select_id.load(SeqCst)) == 0 {
        let now = Instant::now();
        if let Some(end) = deadline {
            if now < end {
                thread::park_timeout(end - now);
            } else if ACTOR.with(|a| a.select_id.compare_and_swap(0, 1, SeqCst)) == 0 {
                return false;
            }
        } else {
            thread::park();
        }
    }
    true
}

impl Actor {
    pub fn select(&self, id: usize) -> bool {
        self.select_id.compare_and_swap(0, id, SeqCst) == 0
    }

    pub fn unpark(&self) {
        self.thread.unpark();
    }
}
