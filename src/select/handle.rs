use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use select::CaseId;
use utils::Backoff;

pub struct Inner {
    pub case_id: AtomicUsize,
    pub thread: Thread,
    /// A slot into which another thread may store a pointer to its `Request`.
    pub request_ptr: AtomicUsize,
}

#[derive(Clone)]
pub struct Handle {
    pub inner: Arc<Inner>,
}

impl Handle {
    pub fn try_select(&self, case_id: CaseId) -> bool {
        self.inner
            .case_id
            .compare_and_swap(CaseId::none().into(), case_id.into(), SeqCst) == CaseId::none().into()
    }

    pub fn unpark(&self) {
        self.inner.thread.unpark();
    }

    pub fn thread_id(&self) -> ThreadId {
        self.inner.thread.id()
    }

    pub fn reset(&self) {
        self.inner.case_id.store(0, SeqCst);
    }

    pub fn selected(&self) -> CaseId {
        CaseId::from(self.inner.case_id.load(SeqCst))
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
    pub static HANDLE: Handle = Handle {
        inner: Arc::new(Inner {
            case_id: AtomicUsize::new(CaseId::none().into()),
            thread: thread::current(),
            request_ptr: AtomicUsize::new(0),
        })
    };
}

pub fn current() -> Handle {
    HANDLE.with(|a| a.clone())
}

pub fn current_try_select(case_id: CaseId) -> bool {
    HANDLE.with(|a| a.try_select(case_id))
}

pub fn current_selected() -> CaseId {
    HANDLE.with(|a| a.selected())
}

pub fn current_reset() {
    HANDLE.with(|a| a.reset())
}

pub fn current_wait_until(deadline: Option<Instant>) -> bool {
    HANDLE.with(|a| a.wait_until(deadline))
}
