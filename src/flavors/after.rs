use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::thread;
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
        let flag = self.flag();

        let arc = unsafe { Arc::from_raw(&flag) };
        mem::forget(arc.clone());
        mem::forget(arc);
        Channel {
            deadline: self.deadline,
            ptr: AtomicPtr::new(flag as *const _ as *mut _),
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
    pub fn channel_id(&self) -> usize {
        self.flag() as *const _ as usize
    }

    #[inline]
    pub unsafe fn read(&self, token: &mut Token) -> Option<Instant> {
        token.after
    }

    #[inline]
    fn flag(&self) -> &AtomicBool {
        unsafe {
            let mut ptr = self.ptr.load(Ordering::SeqCst);
            if ptr.is_null() {
                ptr = Arc::into_raw(Arc::new(AtomicBool::new(false))) as *mut _;

                if let Err(p) = self.ptr.compare_exchange(ptr::null_mut(), ptr, Ordering::SeqCst, Ordering::SeqCst) {
                    drop(Arc::from_raw(ptr));
                    ptr = p;
                }
            }
            &*(ptr as *const AtomicBool)
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
        // if let Some(flag) = self.try_get() {
        //     if flag.load(Ordering::SeqCst) {
        //         return None;
        //     }
        // }

        let mut now;
        loop {
            now = Instant::now();
            if now >= self.deadline {
                break;
            }
            thread::sleep(self.deadline - now);
        }

        if !self.flag().swap(true, Ordering::SeqCst) {
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

        !self.flag().load(Ordering::SeqCst)
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
            if self.flag().load(Ordering::SeqCst) {
                *token = None;
                return true;
            }
        }

        let now = Instant::now();
        if now < self.deadline {
            return false;
        }

        if !self.flag().swap(true, Ordering::SeqCst) {
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
