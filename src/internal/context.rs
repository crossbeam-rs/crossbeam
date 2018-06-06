use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use internal::select::CaseId;
use internal::utils::Backoff;

// TODO: explain all orderings here

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
        if self
            .case_id
            .compare_and_swap(CaseId::Waiting.into(), case_id.into(), Ordering::Relaxed)
            == CaseId::Waiting.into()
        {
            self.packet.store(packet, Ordering::Release);
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn try_abort(&self) -> CaseId {
        match self
            .case_id
            .compare_exchange(
                CaseId::Waiting.into(),
                CaseId::Aborted.into(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
        {
            Ok(_) => CaseId::Aborted,
            Err(id) => CaseId::from(id),
        }
    }

    #[inline]
    pub fn unpark(&self) {
        self.thread.unpark();
    }

    #[inline]
    pub fn reset(&self) {
        // Using relaxed orderings is safe because these store operations will be visibile to other
        // threads only through a waker wrapped within a mutex.
        self.case_id.store(0, Ordering::Relaxed);
        self.packet.store(0, Ordering::Relaxed);
    }

    #[inline]
    pub fn selected(&self) -> CaseId {
        CaseId::from(self.case_id.load(Ordering::Acquire))
    }

    #[inline]
    pub fn wait_until(&self, deadline: Option<Instant>) -> CaseId {
        let backoff = &mut Backoff::new();
        loop {
            let sel = self.selected();
            if sel != CaseId::Waiting {
                return sel;
            }

            if !backoff.step() {
                break;
            }
        }

        loop {
            let sel = self.selected();
            if sel != CaseId::Waiting {
                return sel;
            }

            if let Some(end) = deadline {
                let now = Instant::now();

                if now < end {
                    thread::park_timeout(end - now);
                } else {
                    return self.try_abort();
                }
            } else {
                thread::park();
            }
        }
    }

    #[inline]
    fn wait_packet(&self) -> usize {
        let backoff = &mut Backoff::new();
        loop {
            let packet = self.packet.load(Ordering::Acquire);
            if packet != 0 {
                return packet;
            }
            backoff.step();
        }
    }
}

thread_local! {
    pub static CONTEXT: Arc<Context> = Arc::new(Context {
        case_id: AtomicUsize::new(CaseId::Waiting.into()),
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
pub fn current_try_abort() -> CaseId {
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
pub fn current_wait_until(deadline: Option<Instant>) -> CaseId {
    CONTEXT.with(|c| c.wait_until(deadline))
}

#[inline]
pub fn current_thread_id() -> ThreadId {
    CONTEXT.with(|c| c.thread_id)
}

#[inline]
pub fn current_wait_packet() -> usize {
    CONTEXT.with(|c| c.wait_packet())
}
