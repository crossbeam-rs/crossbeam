use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use backoff::Backoff;
use select::CaseId;

pub struct Actor {
    case_id: AtomicUsize,
    thread: Thread,
}

impl Actor {
    pub fn select(&self, case_id: CaseId) -> bool {
        self.case_id
            .compare_and_swap(CaseId::none().id, case_id.id, SeqCst) == CaseId::none().id
    }

    pub fn unpark(&self) {
        self.thread.unpark();
    }

    pub fn thread_id(&self) -> ThreadId {
        self.thread.id()
    }

    pub fn reset(&self) {
        self.case_id.store(0, SeqCst);
    }

    pub fn selected(&self) -> CaseId {
        CaseId::new(self.case_id.load(SeqCst))
    }

    pub fn wait_until(&self, deadline: Option<Instant>) -> bool {
        let mut backoff = Backoff::new();
        loop {
            if self.selected() != CaseId::none() {
                return true;
            }
            if !backoff.step() {
                break;
            }
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
}

thread_local! {
    static ACTOR: Arc<Actor> = Arc::new(Actor {
        case_id: AtomicUsize::new(CaseId::none().id),
        thread: thread::current(),
    });
}

pub fn current() -> Arc<Actor> {
    ACTOR.with(|a| a.clone())
}

pub fn current_select(case_id: CaseId) -> bool {
    ACTOR.with(|a| a.select(case_id))
}

pub fn current_selected() -> CaseId {
    ACTOR.with(|a| a.selected())
}

pub fn current_reset() {
    ACTOR.with(|a| a.reset())
}

pub fn current_wait_until(deadline: Option<Instant>) -> bool {
    ACTOR.with(|a| a.wait_until(deadline))
}
