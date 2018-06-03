//! A channel that delivers a message after a certain amount of time.
//!
//! Messages cannot be sent in this kind of channel; they appear implicitly.

use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use internal::channel::RecvNonblocking;
use internal::select::{CaseId, Select, Token};
use internal::utils;

/// Result of the receive operation.
pub type AfterToken = Option<Instant>;

/// A channel that delivers a message after a certain amount of time.
pub struct Channel {
    /// The instant at which the message will be delivered.
    deadline: Instant,

    /// The pointer to a lazily initialized boolean flag, which becomes `true` when the message
    /// gets consumed.
    ///
    /// This `AtomicPtr` holds the raw value of an `Arc<AtomicBool>`.
    // TODO: Use `AtomicPtr<AtomicBool>` here once we implement `AtomicCell`.
    ptr: AtomicPtr<AtomicBool>,
}

impl Channel {
    /// Creates a channel that delivers a message after a certain duration of time.
    #[inline]
    pub fn new(dur: Duration) -> Self {
        Channel {
            deadline: Instant::now() + dur,
            ptr: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Returns a unique identifier for the channel.
    #[inline]
    pub fn channel_id(&self) -> usize {
        self.flag() as *const AtomicBool as usize
    }

    /// Returns the flag associated with this channel.
    ///
    /// The flag will be allocated on the heap and initialized with `false` on the first call of
    /// this method.
    #[inline]
    fn flag(&self) -> &AtomicBool {
        let mut ptr = self.ptr.load(Ordering::Acquire);
        loop {
            if !ptr.is_null() {
                return unsafe { &*(ptr as *const AtomicBool) };
            }

            // Try initializing the flag.
            let new = Arc::into_raw(Arc::new(AtomicBool::new(false))) as *mut AtomicBool;
            let old = self.ptr.compare_and_swap(ptr::null_mut(), new, Ordering::AcqRel);

            if old.is_null() {
                // The flag was successfully initialized.
                ptr = new;
            } else {
                // Another thread has initialized the flag before us.
                ptr = old;
                unsafe { drop(Arc::from_raw(new)) }
            }
        }
    }

    /// Receives a message from the channel.
    #[inline]
    pub fn recv(&self) -> Option<Instant> {
        if self.flag().load(Ordering::SeqCst) {
            // If the message was already consumed, another one will never come.
            utils::sleep_forever();
        }

        // Wait until the deadline.
        loop {
            let now = Instant::now();
            if now >= self.deadline {
                break;
            }
            thread::sleep(self.deadline - now);
        }

        // Try consuming the message if it is still available.
        if !self.flag().swap(true, Ordering::SeqCst) {
            // Success! Return the message, which is the instant at which it was "sent".
            Some(self.deadline)
        } else {
            // The message was already consumed, and another one will never come.
            utils::sleep_forever();
        }
    }

    /// Attempts to receive a message without blocking.
    #[inline]
    pub fn recv_nonblocking(&self) -> RecvNonblocking<Instant> {
        // We use relaxed ordering because this is just an optional, optimistic check.
        if !self.ptr.load(Ordering::Relaxed).is_null() && self.flag().load(Ordering::SeqCst) {
            // The message was already consumed.
            return RecvNonblocking::Empty;
        }

        if Instant::now() < self.deadline {
            // The message was not delivered yet.
            return RecvNonblocking::Empty;
        }

        // Try consuming the message if it is still available.
        if !self.flag().swap(true, Ordering::SeqCst) {
            // Success! Return the message, which is the instant at which it was "sent".
            RecvNonblocking::Message(self.deadline)
        } else {
            // The message was already consumed.
            RecvNonblocking::Empty
        }
    }

    /// Reads a message from the channel.
    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Option<Instant> {
        token.after
    }

    /// Returns `true` if the channel is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        let flag = self.flag();

        // First, check if the message was already consumed to avoid the expensive
        // `Instant::now()` call.
        if flag.load(Ordering::SeqCst) {
            return true;
        }

        // If the deadline hasn't been reached yet, the channel is empty.
        if Instant::now() < self.deadline {
            return true;
        }

        // The deadline has been reached. The channel is empty only if the message was consumed.
        flag.load(Ordering::SeqCst)
    }

    /// Returns the number of messages in the channel.
    #[inline]
    pub fn len(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            1
        }
    }
}

impl Drop for Channel {
    #[inline]
    fn drop(&mut self) {
        let ptr = self.ptr.load(Ordering::SeqCst);
        if !ptr.is_null() {
            unsafe { drop(Arc::from_raw(ptr)); }
        }
    }
}

impl Clone for Channel {
    #[inline]
    fn clone(&self) -> Channel {
        let flag = self.flag();

        // Increment the reference count.
        let arc = unsafe { Arc::from_raw(flag as *const AtomicBool as *mut AtomicBool) };
        mem::forget(arc.clone());
        mem::forget(arc);

        Channel {
            deadline: self.deadline,
            ptr: AtomicPtr::new(flag as *const AtomicBool as *mut AtomicBool),
        }
    }
}

impl Select for Channel {
    #[inline]
    fn try(&self, token: &mut Token) -> bool {
        match self.recv_nonblocking() {
            RecvNonblocking::Message(msg) => {
                token.after = Some(msg);
                true
            }
            RecvNonblocking::Closed => {
                token.after = None;
                true
            }
            RecvNonblocking::Empty => {
                false
            }
        }
    }

    #[inline]
    fn retry(&self, token: &mut Token) -> bool {
        self.try(token)
    }

    #[inline]
    fn deadline(&self) -> Option<Instant> {
        Some(self.deadline)
    }

    #[inline]
    fn register(&self, _token: &mut Token, _case_id: CaseId) -> bool {
        true
    }

    #[inline]
    fn unregister(&self, _case_id: CaseId) {}

    #[inline]
    fn accept(&self, token: &mut Token) -> bool {
        self.try(token)
    }
}
