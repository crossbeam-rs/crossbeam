use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use CaseId;

pub(crate) struct Actor {
    case_id: AtomicUsize,
    thread: Thread,
}

impl Actor {
    pub fn select(&self, case_id: CaseId) -> bool {
        self.case_id
            .compare_and_swap(CaseId::none().0, case_id.0, SeqCst) == CaseId::none().0
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
        CaseId(self.case_id.load(SeqCst))
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
}

thread_local! {
    static ACTOR: Arc<Actor> = Arc::new(Actor {
        case_id: AtomicUsize::new(CaseId::none().0),
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
