use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use select::CaseId;
use utils::Backoff;

pub struct Context {
    pub case_id: AtomicUsize,
    pub thread: Thread,
    /// A slot into which another thread may store a pointer to its `Request`.
    pub request_ptr: AtomicUsize,
}

impl Context {
    pub fn try_select(&self, case_id: CaseId) -> bool {
        self.case_id
            .compare_and_swap(CaseId::none().into(), case_id.into(), Ordering::SeqCst) == CaseId::none().into()
    }

    pub fn unpark(&self) {
        self.thread.unpark();
    }

    pub fn reset(&self) {
        self.case_id.store(0, Ordering::SeqCst);
    }

    pub fn selected(&self) -> CaseId {
        CaseId::from(self.case_id.load(Ordering::SeqCst))
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
                } else if self.try_select(CaseId::abort()) {
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
    pub static CONTEXT: Arc<Context> = Arc::new(Context {
        case_id: AtomicUsize::new(CaseId::none().into()),
        thread: thread::current(),
        request_ptr: AtomicUsize::new(0),
    });
}

pub fn current() -> Arc<Context> {
    CONTEXT.with(|h| h.clone())
}

pub fn current_try_select(case_id: CaseId) -> bool {
    CONTEXT.with(|h| h.try_select(case_id))
}

pub fn current_selected() -> CaseId {
    CONTEXT.with(|h| h.selected())
}

pub fn current_reset() {
    CONTEXT.with(|h| h.reset())
}

pub fn current_wait_until(deadline: Option<Instant>) -> bool {
    CONTEXT.with(|h| h.wait_until(deadline))
}
