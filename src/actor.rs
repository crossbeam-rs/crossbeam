use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
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
        self.select_id
            .compare_and_swap(CaseId::none().0, case_id.0, SeqCst) == CaseId::none().0
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
        CaseId(self.select_id.load(SeqCst))
    }

    pub fn wait_until(&self, deadline: Option<Instant>) -> bool {
        for i in 0..10 {
            if self.selected() != CaseId::none() {
                return true;
            }
            for _ in 0..1 << i {
                // ::std::sync::atomic::hint_core_should_pause();
            }
        }

        for _ in 0..10 {
            if self.selected() != CaseId::none() {
                return true;
            }
            thread::yield_now();
        }

        while self.selected() == CaseId::none() {
            let now = Instant::now();

            if let Some(end) = deadline {
                if now < end {
                    thread::park_timeout(end - now);
                } else if self.select(CaseId::abort()) {
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
        (*req).actor.select(CaseId::abort());
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
        let v = (*req).packet.take();
        (*req).actor.select(CaseId::abort());
        thread.unpark();
        v
    }

    pub fn recv<T>(&self) -> T {
        current().reset();
        let req = Request::new(None);
        self.request_ptr.store(&req as *const _ as usize, SeqCst);
        self.unpark();
        current().wait_until(None);
        req.packet.take()
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
        select_id: AtomicUsize::new(CaseId::none().0),
        request_ptr: AtomicUsize::new(0),
        thread: thread::current(),
    });
}

pub(crate) fn current() -> Arc<Actor> {
    ACTOR.with(|a| a.clone())
}

pub(crate) fn current_select(case_id: CaseId) -> bool {
    ACTOR.with(|a| a.select(case_id))
}

pub(crate) fn current_selected() -> CaseId {
    ACTOR.with(|a| a.selected())
}

pub(crate) fn current_reset() {
    ACTOR.with(|a| a.reset())
}

pub(crate) fn current_wait_until(deadline: Option<Instant>) -> bool {
    ACTOR.with(|a| a.wait_until(deadline))
}

pub(crate) struct Packet<T>(Mutex<Option<T>>, AtomicBool);

impl<T> Packet<T> {
    pub fn new(data: Option<T>) -> Self {
        Packet(Mutex::new(data), AtomicBool::new(false))
    }

    pub fn put(&self, data: T) {
        {
        let mut opt = self.0.try_lock().unwrap();
        assert!(opt.is_none());
        *opt = Some(data);
        }
        self.1.store(true, SeqCst);
    }

    pub fn take(&self) -> T {
        let t = self.0.try_lock().unwrap().take().unwrap();
        self.1.store(true, SeqCst);
        t
    }

    pub fn wait(&self) {
        for i in 0..10 {
            if self.1.load(SeqCst) {
                return;
            }
            for _ in 0..1 << i {
                // ::std::sync::atomic::hint_core_should_pause();
            }
        }

        loop {
            if self.1.load(SeqCst) {
                return;
            }
            thread::yield_now();
        }
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
