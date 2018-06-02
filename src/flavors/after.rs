use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use internal::select::CaseId;
use internal::select::Select;
use internal::select::Token;
use internal::utils;

pub type AfterToken = Option<Instant>;

pub struct Channel {
    deadline: Instant,
    ptr: AtomicPtr<AtomicBool>,
}

impl Clone for Channel {
    #[inline]
    fn clone(&self) -> Channel {
        let flag = self.flag();

        let arc = unsafe { Arc::from_raw(flag as *const AtomicBool as *mut AtomicBool) };
        mem::forget(arc.clone());
        mem::forget(arc);

        Channel {
            deadline: self.deadline,
            ptr: AtomicPtr::new(flag as *const AtomicBool as *mut AtomicBool),
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

impl Channel {
    #[inline]
    pub fn channel_id(&self) -> usize {
        self.flag() as *const _ as usize
    }

    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Option<Instant> {
        token.after
    }

    #[inline]
    fn flag(&self) -> &AtomicBool {
        loop {
            let ptr = self.ptr.load(Ordering::SeqCst);
            if !ptr.is_null() {
                return unsafe { &*(ptr as *const AtomicBool) };
            }

            let new = Arc::into_raw(Arc::new(AtomicBool::new(false))) as *mut AtomicBool;
            if !self.ptr.compare_and_swap(ptr::null_mut(), new, Ordering::SeqCst).is_null() {
                unsafe { drop(Arc::from_raw(new)) }
            }
        }
    }
}

impl Channel {
    #[inline]
    pub fn new(dur: Duration) -> Self {
        Channel {
            deadline: Instant::now() + dur,
            ptr: AtomicPtr::new(ptr::null_mut()),
        }
    }

    #[inline]
    pub fn recv(&self) -> Option<Instant> {
        if self.flag().load(Ordering::SeqCst) {
            utils::sleep_forever();
        }

        loop {
            let now = Instant::now();
            if now >= self.deadline {
                break;
            }
            thread::sleep(self.deadline - now);
        }

        if !self.flag().swap(true, Ordering::SeqCst) {
            Some(self.deadline)
        } else {
            utils::sleep_forever();
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.flag().load(Ordering::SeqCst) || Instant::now() < self.deadline
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
        let token = &mut token.after;

        if !self.ptr.load(Ordering::SeqCst).is_null() && self.flag().load(Ordering::SeqCst) {
            return false;
        }

        let now = Instant::now();
        if now < self.deadline {
            return false;
        }

        if self.flag().swap(true, Ordering::SeqCst) {
            return false;
        }

        *token = Some(self.deadline);
        true
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
