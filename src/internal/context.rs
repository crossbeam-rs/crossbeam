//! Thread-local context used in select.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, Thread, ThreadId};
use std::time::Instant;

use internal::select::Selected;
use internal::utils::Backoff;

/// Thread-local context used in select.
///
/// This struct is typically wrapped in an `Arc` so that it can be shared among other threads, too.
pub struct Context {
    /// Selected operation.
    select: AtomicUsize,

    /// Thread handle.
    thread: Thread,

    /// Thread id.
    thread_id: ThreadId,

    /// A slot into which another thread may store a pointer to its `Packet`.
    packet: AtomicUsize,
}

impl Context {
    /// Attempts to select an operation.
    ///
    /// On failure, the previously selected operation is returned.
    #[inline]
    pub fn try_select(&self, select: Selected) -> Result<(), Selected> {
        self.select
            .compare_exchange(
                Selected::Waiting.into(),
                select.into(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|e| e.into())
    }

    /// Returns the selected operation.
    #[inline]
    pub fn selected(&self) -> Selected {
        Selected::from(self.select.load(Ordering::Acquire))
    }

    /// Stores a packet.
    ///
    /// This method must be called after `try_select` succeeds and there is a packet to provide.
    #[inline]
    pub fn store_packet(&self, packet: usize) {
        if packet != 0 {
            self.packet.store(packet, Ordering::Release);
        }
    }

    /// Unparks the thread this context belongs to.
    #[inline]
    pub fn unpark(&self) {
        self.thread.unpark();
    }

    /// Returns the id of the thread this context belongs to.
    #[inline]
    pub fn thread_id(&self) -> ThreadId {
        self.thread_id
    }
}

thread_local! {
    /// The thread-local context.
    static CONTEXT: Arc<Context> = Arc::new(Context {
        select: AtomicUsize::new(Selected::Waiting.into()),
        thread: thread::current(),
        thread_id: thread::current().id(),
        packet: AtomicUsize::new(0),
    });
}

/// Returns the context associated with the current thread.
#[inline]
pub fn current() -> Arc<Context> {
    CONTEXT.with(|cx| cx.clone())
}

/// Attempts to select an operation for the current thread.
#[inline]
pub fn current_try_select(select: Selected) -> Result<(), Selected> {
    CONTEXT.with(|cx| cx.try_select(select))
}

/// Returns the selected operation for the current thread.
#[inline]
pub fn current_selected() -> Selected {
    CONTEXT.with(|cx| cx.selected())
}

/// Resets `select` and `packet`.
///
/// This method is used for initialization before the start of select.
#[inline]
pub fn current_reset() {
    CONTEXT.with(|cx| {
        cx.select.store(Selected::Waiting.into(), Ordering::Release);
        cx.packet.store(0, Ordering::Release);
    })
}

/// Waits until an operation is selected for the current thread and returns it.
///
/// If the deadline is reached, `Selected::Aborted` will be selected.
#[inline]
pub fn current_wait_until(deadline: Option<Instant>) -> Selected {
    CONTEXT.with(|cx| {
        // Spin for a short time, waiting until an operation is selected.
        let backoff = &mut Backoff::new();
        loop {
            let sel = Selected::from(cx.select.load(Ordering::Acquire));
            if sel != Selected::Waiting {
                return sel;
            }

            if !backoff.step() {
                break;
            }
        }

        loop {
            // Check whether an operation has been selected.
            let sel = Selected::from(cx.select.load(Ordering::Acquire));
            if sel != Selected::Waiting {
                return sel;
            }

            // If there's a deadline, park the current thread until the deadline is reached.
            if let Some(end) = deadline {
                let now = Instant::now();

                if now < end {
                    thread::park_timeout(end - now);
                } else {
                    // The deadline has been reached. Try aborting select.
                    return match cx.try_select(Selected::Aborted) {
                        Ok(()) => Selected::Aborted,
                        Err(s) => s,
                    };
                }
            } else {
                thread::park();
            }
        }
    })
}

/// Waits until a packet is provided to the current thread and returns it.
#[inline]
pub fn current_wait_packet() -> usize {
    CONTEXT.with(|cx| {
        let backoff = &mut Backoff::new();
        loop {
            let packet = cx.packet.load(Ordering::Acquire);
            if packet != 0 {
                return packet;
            }
            backoff.step();
        }
    })
}

/// Returns the id of the current thread.
#[inline]
pub fn current_thread_id() -> ThreadId {
    CONTEXT.try_with(|cx| cx.thread_id)
        .unwrap_or_else(|_| thread::current().id())
}
