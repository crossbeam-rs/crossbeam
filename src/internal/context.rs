use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use internal::select::CaseId;
use internal::utils::Backoff;

pub struct Context {
    pub case_id: AtomicUsize,
    pub thread: Thread,
    pub thread_id: ThreadId,
    /// A slot into which another thread may store a pointer to its `Request`.
    pub packet: AtomicUsize,
}

impl Context {
    #[inline]
    pub fn try_select(&self, case_id: CaseId, packet: usize) -> bool {
        if self.case_id
            .compare_and_swap(CaseId::none().into(), case_id.into(), Ordering::SeqCst) == CaseId::none().into() {
            self.packet.store(packet, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn try_abort(&self) -> bool {
        self.case_id
            .compare_and_swap(CaseId::none().into(), CaseId::abort().into(), Ordering::SeqCst) == CaseId::none().into()
    }

    #[inline]
    pub fn unpark(&self) {
        self.thread.unpark();
    }

    #[inline]
    pub fn reset(&self) {
        // TODO: Using relaxed here would make spsc in bounded case much faster
        self.case_id.store(0, Ordering::SeqCst);
        self.packet.store(0, Ordering::SeqCst);
    }

    #[inline]
    pub fn selected(&self) -> CaseId {
        CaseId::from(self.case_id.load(Ordering::SeqCst))
    }

    #[inline]
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
            if let Some(end) = deadline {
                let now = Instant::now();
                if now < end {
                    thread::park_timeout(end - now);
                } else if self.try_abort() {
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
        thread_id: thread::current().id(),
        packet: AtomicUsize::new(0),
    });
}

#[inline]
pub fn current() -> Arc<Context> {
    CONTEXT.with(|c| c.clone())
}

#[inline]
// pub fn current_try_select(case_id: CaseId) -> bool {
//     CONTEXT.with(|c| c.try_select(case_id))
// }

#[inline]
pub fn current_try_abort() -> bool {
    CONTEXT.with(|c| c.try_abort())
}

#[inline]
pub fn current_selected() -> CaseId {
    CONTEXT.with(|c| c.selected())
}

#[inline]
pub fn current_reset() {
    CONTEXT.with(|c| c.reset())
}

#[inline]
pub fn current_wait_until(deadline: Option<Instant>) -> bool {
    CONTEXT.with(|c| c.wait_until(deadline))
}

#[inline]
pub fn current_thread_id() -> ThreadId {
    CONTEXT.with(|c| c.thread_id)
}
