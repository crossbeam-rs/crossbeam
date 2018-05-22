use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};

use internal::select::CaseId;
use internal::select::Select;
use internal::select::Token;

pub type AfterToken = Option<Instant>;

pub struct Channel {
    deadline: Instant,
    ptr: AtomicPtr<AtomicBool>,
}

impl Clone for Channel {
    #[inline]
    fn clone(&self) -> Channel {
        self.upgrade();

        let ptr = self.ptr.load(Ordering::SeqCst);
        let arc = unsafe { Arc::from_raw(ptr) };
        mem::forget(arc.clone());
        mem::forget(arc);
        Channel {
            deadline: self.deadline,
            ptr: AtomicPtr::new(ptr),
        }
    }
}

impl Drop for Channel {
    #[inline]
    fn drop(&mut self) {
        let ptr = self.ptr.load(Ordering::SeqCst);
        if !ptr.is_null() {
            unsafe { Arc::from_raw(ptr); }
        }
    }
}

impl Channel {
    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Option<Instant> {
        token.after
    }

    #[inline]
    fn upgrade(&self) {
        if self.ptr.load(Ordering::SeqCst).is_null() {
            let ptr = Arc::into_raw(Arc::new(AtomicBool::new(false))) as *mut _;

            if !self.ptr.compare_and_swap(ptr::null_mut(), ptr, Ordering::SeqCst).is_null() {
                unsafe { Arc::from_raw(ptr); }
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
        if self.ptr.load(Ordering::SeqCst).is_null() {
            return None;
        }

        let mut now;
        loop {
            now = Instant::now();
            if now >= self.deadline {
                break;
            }
            ::std::thread::sleep(self.deadline - now);
        }

        self.upgrade();
        let ptr = self.ptr.load(Ordering::SeqCst);

        if unsafe { !(*ptr).swap(true, Ordering::SeqCst) } {
            Some(now)
        } else {
            None
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        if Instant::now() < self.deadline {
            return true;
        }

        self.upgrade();
        let ptr = self.ptr.load(Ordering::SeqCst);

        unsafe { !(*ptr).load(Ordering::SeqCst) }
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

        let ptr = self.ptr.load(Ordering::SeqCst);
        if !ptr.is_null() {
            if unsafe { (*ptr).load(Ordering::SeqCst) } {
                *token = None;
                return true;
            }
        }

        let now = Instant::now();
        if now < self.deadline {
            return false;
        }

        if ptr.is_null() {
            self.upgrade();
        }
        let ptr = self.ptr.load(Ordering::SeqCst);

        if unsafe { !(*ptr).swap(true, Ordering::SeqCst) } {
            *token = Some(now);
        } else {
            *token = None;
        }
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
