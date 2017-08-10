use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, AtomicBool};
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use parking_lot::Mutex;

use CaseId;

pub(crate) struct Actor {
    select_id: AtomicUsize,
    request_ptr: AtomicUsize,
    thread: Thread,
}

impl Actor {
    pub fn select(&self, case_id: CaseId) -> bool {
        self.select_id.compare_and_swap(0, case_id.into_usize(), SeqCst) == 0
    }

    pub fn unpark(&self) {
        self.thread.unpark();
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread.id()
    }

    pub fn reset(&self) {
        self.select_id.store(0, SeqCst);
        self.request_ptr.store(0, SeqCst);
    }

    pub fn selected(&self) -> CaseId {
        CaseId::from_usize(self.select_id.load(SeqCst))
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
                } else if self.select(CaseId::none()) {
                    return false;
                }
            } else {
                thread::park();
            }
        }

        true
    }

    pub unsafe fn finish_send<T>(&self, value: T) {
        let req = loop {
            let ptr = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
            if !ptr.is_null() {
                break ptr;
            }
            thread::yield_now();
        };

        let thread = (*req).actor.thread.clone();
        (*req).packet.put(value);
        (*req).actor.select(CaseId::none());
        thread.unpark();
    }

    pub unsafe fn finish_recv<T>(&self) -> T {
        let req = loop {
            let ptr = self.request_ptr.swap(0, SeqCst) as *const Request<T>;
            if !ptr.is_null() {
                break ptr;
            }
            thread::yield_now();
        };

        let thread = (*req).actor.thread.clone();
        let v = (*req).packet.take().unwrap();
        (*req).actor.select(CaseId::none());
        thread.unpark();
        v
    }

    pub fn recv<T>(&self) -> T {
        current().reset();
        let req = Request::new(None);
        self.request_ptr.store(&req as *const _ as usize, SeqCst);
        self.unpark();
        current().wait_until(None);
        req.packet.take().unwrap()
    }

    pub fn send<T>(&self, value: T) {
        current().reset();
        let req = Request::new(Some(value));
        self.request_ptr.store(&req as *const _ as usize, SeqCst);
        self.unpark();
        current().wait_until(None);
    }
}

thread_local! {
    static ACTOR: Arc<Actor> = Arc::new(Actor {
        select_id: AtomicUsize::new(0),
        request_ptr: AtomicUsize::new(0),
        thread: thread::current(),
    });
}

pub(crate) fn current() -> Arc<Actor> {
    ACTOR.with(|a| a.clone())
}

pub(crate) struct Packet<T>(Mutex<Option<T>>);

impl<T> Packet<T> {
    pub fn new(data: Option<T>) -> Self {
        Packet(Mutex::new(data))
    }

    pub fn put(&self, data: T) {
        let mut opt = self.0.try_lock().unwrap();
        assert!(opt.is_none());
        *opt = Some(data);
    }

    pub fn take(&self) -> Option<T> {
        self.0.try_lock().unwrap().take()
    }
}

struct Request<T> {
    actor: Arc<Actor>,
    packet: Packet<T>,
}

impl<T> Request<T> {
    pub fn new(data: Option<T>) -> Self {
        Request {
            actor: current(),
            packet: Packet::new(data),
        }
    }
}
