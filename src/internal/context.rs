use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use internal::select::Select;
use internal::utils::Backoff;

// TODO: explain all orderings here

pub struct Context {
    pub select: AtomicUsize,
    pub thread: Thread,
    pub thread_id: ThreadId,
    /// A slot into which another thread may store a pointer to its `Request`.
    pub packet: AtomicUsize,
}

impl Context {
    // TODO: return Result<(), Select>?
    #[inline]
    pub fn try_select(&self, select: Select, packet: usize) -> bool {
        if self
            .select
            .compare_and_swap(Select::Waiting.into(), select.into(), Ordering::Relaxed)
            == Select::Waiting.into()
        {
            self.packet.store(packet, Ordering::Release);
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn try_abort(&self) -> Select {
        match self
            .select
            .compare_exchange(
                Select::Waiting.into(),
                Select::Aborted.into(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
        {
            Ok(_) => Select::Aborted,
            Err(id) => Select::from(id),
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
        self.select.store(0, Ordering::Relaxed);
        self.packet.store(0, Ordering::Relaxed);
    }

    #[inline]
    pub fn selected(&self) -> Select {
        Select::from(self.select.load(Ordering::Acquire))
    }

    #[inline]
    pub fn wait_until(&self, deadline: Option<Instant>) -> Select {
        let backoff = &mut Backoff::new();
        loop {
            let sel = self.selected();
            if sel != Select::Waiting {
                return sel;
            }

            if !backoff.step() {
                break;
            }
        }

        loop {
            let sel = self.selected();
            if sel != Select::Waiting {
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
        select: AtomicUsize::new(Select::Waiting.into()),
        thread: thread::current(),
        thread_id: thread::current().id(),
        packet: AtomicUsize::new(0),
    });
}

#[inline]
pub fn current() -> Arc<Context> {
    CONTEXT.with(|c| c.clone())
}

// TODO: replace with try_select(Select::Aborted, 0)?
#[inline]
pub fn current_try_abort() -> Select {
    CONTEXT.with(|c| c.try_abort())
}

#[inline]
pub fn current_selected() -> Select {
    CONTEXT.with(|c| c.selected())
}

#[inline]
pub fn current_reset() {
    CONTEXT.with(|c| c.reset())
}

#[inline]
pub fn current_wait_until(deadline: Option<Instant>) -> Select {
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
