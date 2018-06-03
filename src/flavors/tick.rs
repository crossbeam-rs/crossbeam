use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use internal::channel::RecvNonblocking;
use internal::select::CaseId;
use internal::select::Select;
use internal::select::Token;

pub type TickToken = Option<Instant>;

pub struct Channel {
    // TODO: Use `Arc<AtomicCell<Instant>>` here once we implement `AtomicCell`.
    deadline: Arc<Mutex<Instant>>,
    duration: Duration,
}

impl Clone for Channel {
    #[inline]
    fn clone(&self) -> Channel {
        Channel {
            deadline: self.deadline.clone(),
            duration: self.duration,
        }
    }
}

impl Channel {
    #[inline]
    pub fn channel_id(&self) -> usize {
        self.deadline.as_ref() as *const Mutex<Instant> as usize
    }

    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Option<Instant> {
        token.tick
    }
}

impl Channel {
    #[inline]
    pub fn new(dur: Duration) -> Self {
        Channel {
            deadline: Arc::new(Mutex::new(Instant::now() + dur)),
            duration: dur,
        }
    }

    #[inline]
    pub fn recv(&self) -> Option<Instant> {
        loop {
            let offset = {
                let mut deadline = self.deadline.lock();
                let now = Instant::now();

                if now >= *deadline {
                    let msg = Some(*deadline);
                    *deadline = now + self.duration;
                    return msg;
                }

                *deadline - now
            };

            thread::sleep(offset);
        }
    }

    #[inline]
    pub fn recv_nonblocking(&self) -> RecvNonblocking<Instant> {
        let mut deadline = self.deadline.lock();
        let now = Instant::now();

        if now >= *deadline {
            let msg = RecvNonblocking::Message(*deadline);
            *deadline = now + self.duration;
            msg
        } else {
            RecvNonblocking::Empty
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        let deadline = *self.deadline.lock();
        Instant::now() < deadline
    }

    #[inline]
    pub fn len(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            1
        }
    }
}

impl Select for Channel {
    #[inline]
    fn try(&self, token: &mut Token) -> bool {
        let token = &mut token.tick;

        let mut deadline = self.deadline.lock();
        let now = Instant::now();

        if now < *deadline {
            return false;
        }

        *token = Some(*deadline);
        *deadline = now + self.duration;
        true
    }

    #[inline]
    fn retry(&self, token: &mut Token) -> bool {
        self.try(token)
    }

    #[inline]
    fn deadline(&self) -> Option<Instant> {
        Some(*self.deadline.lock())
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
