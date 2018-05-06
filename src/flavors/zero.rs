//! Zero-capacity channel.
//!
//! Also known as *rendezvous* channel.

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem;
use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;

use select::CaseId;
use select::Sel;
use context::{self, CONTEXT, Context};
use utils::Backoff;
use waker::{Case, Waker};

struct Entry<T> {
    ready: AtomicUsize,
    msg: UnsafeCell<ManuallyDrop<Option<T>>>, // TODO: does it have to be option?
}

impl<T> Entry<T> {
    fn wait(&self) -> bool {
        let mut backoff = Backoff::new();
        loop {
            let ready = self.ready.load(Ordering::SeqCst);
            if ready == 1 {
                return true;
            }
            if ready == 2 {
                return false;
            }
            backoff.step();
        }
    }
}

/// A zero-capacity channel.
pub struct Channel<T> {
    senders: Waker,
    receivers: Waker,
    is_closed: AtomicBool,
    _marker: PhantomData<T>,
}

impl<T> Channel<T> {
    pub fn receiver(&self) -> Receiver<T> {
        Receiver(self)
    }

    pub fn sender(&self) -> Sender<T> {
        Sender(self)
    }

    // pub fn prepared_sender(&self) -> PreparedSender<T> {
    //     PreparedSender(self)
    // }
    // TODO: remove this
    pub fn prepared_sender(&self) -> Sender<T> {
        Sender(self)
    }

    /// Returns a new zero-capacity channel.
    pub fn new() -> Self {
        Channel {
            senders: Waker::new(),
            receivers: Waker::new(),
            is_closed: AtomicBool::new(false),
            _marker: PhantomData,
        }
    }

    /// Closes the exchanger and wakes up all currently blocked operations on it.
    pub fn close(&self) -> bool {
        if !self.is_closed.swap(true, Ordering::SeqCst) {
            self.senders.abort_all();
            self.receivers.abort_all();
            true
        } else {
            false
        }
    }

    /// Returns `true` if the exchanger is closed.
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::SeqCst)
    }

    pub unsafe fn read(&self, token: &mut Token) -> Option<T> {
        if token.entry == 0 {
            None
        } else {
            let entry = Box::from_raw(token.entry as *mut Entry<T>);
            ManuallyDrop::into_inner(UnsafeCell::into_inner(entry.msg))
        }
    }

    pub unsafe fn write(&self, token: &mut Token, msg: T, is_prepared: bool) {
        let entry = unsafe { &*(token.entry as *const Entry<T>) };
        ptr::write(entry.msg.get(), ManuallyDrop::new(Some(msg)));
        entry.ready.store(1, Ordering::SeqCst);
    }
}

#[derive(Copy, Clone)]
pub struct Token {
    entry: usize,
}

pub struct Receiver<'a, T: 'a>(&'a Channel<T>);
pub struct Sender<'a, T: 'a>(&'a Channel<T>);

impl<'a, T> Sel for Receiver<'a, T> {
    type Token = Token;

    fn try(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        let mut step = 0;
        loop {
            if let Some(case) = self.0.senders.remove_one() {
                case.context.unpark();
                let entry = unsafe { &*(case.payload as *const Entry<T>) };
                if entry.wait() {
                    token.entry = case.payload;
                    return true;
                } else {
                    unsafe {
                        drop(Box::from_raw(case.payload as *mut Entry<T>));
                    }
                }
            }

            if !self.0.is_closed() {
                return false;
            }

            step += 1;
            if step == 2 {
                token.entry = 0;
                return true;
            }
        }
    }

    fn promise(&self, token: &mut Token, case_id: CaseId) {
        let entry = Box::into_raw(Box::new(Entry::<T> {
            ready: AtomicUsize::new(0),
            msg: UnsafeCell::new(ManuallyDrop::new(None)),
        }));
        token.entry = entry as usize;
        self.0.receivers.register_with_payload(case_id, true, entry as usize);
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        !self.0.senders.can_notify() && !self.0.is_closed()
    }

    fn revoke(&self, case_id: CaseId) {
        if let Some(case) = self.0.receivers.unregister(case_id) {
            // TODO: use token.entry instead
            unsafe {
                drop(Box::from_raw(case.payload as *mut Entry<T>));
            }
        }
    }

    fn fulfill(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        let entry = unsafe { &*(token.entry as *const Entry<T>) };
        if entry.wait() {
            true
        } else {
            unsafe {
                drop(Box::from_raw(token.entry as *mut Entry<T>));
            }
            false
        }
    }

    fn finish(&self, token: &mut Token) {}

    fn fail(&self, _token: &mut Token) {}
}

impl<'a, T> Sel for Sender<'a, T> {
    type Token = Token;

    fn try(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        if let Some(case) = self.0.receivers.remove_one() {
            case.context.unpark();
            let entry = unsafe { &*(case.payload as *mut Entry<T>) };
            token.entry = case.payload;
            true
        } else {
            false
        }
    }

    fn promise(&self, token: &mut Token, case_id: CaseId) {
        let entry = Box::into_raw(Box::new(Entry::<T> {
            ready: AtomicUsize::new(0),
            msg: UnsafeCell::new(ManuallyDrop::new(None)),
        }));
        token.entry = entry as usize;
        self.0.senders.register_with_payload(case_id, true, entry as usize);
    }

    fn is_blocked(&self) -> bool {
        // TODO: Add recv_is_blocked() and send_is_blocked() to the three impls
        !self.0.receivers.can_notify()
    }

    fn revoke(&self, case_id: CaseId) {
        self.0.senders.unregister(case_id);
    }

    fn fulfill(&self, token: &mut Token, _backoff: &mut Backoff) -> bool {
        true
    }

    fn finish(&self, token: &mut Token) {}

    fn fail(&self, token: &mut Token) {
        let entry = unsafe { &*(token.entry as *mut Entry<T>) };
        entry.ready.store(2, Ordering::SeqCst);
    }
}
