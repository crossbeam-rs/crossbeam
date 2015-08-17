use std::any::Any;
use std::boxed::FnBox;
use std::cell::RefCell;
use std::mem;
use std::ptr::{self, Unique};
use std::rc::Rc;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Condvar};
use std::thread;

use atomic_option::AtomicOption;

pub unsafe fn spawn_raw<'a, F>(f: F) -> thread::JoinHandle<()> where F: FnOnce() + 'a {
    let closure: Box<FnBox() + 'a> = Box::new(f);
    let closure: Box<FnBox() + Send> = mem::transmute(closure);
    thread::spawn(closure)
}

pub struct Scope<'a> {
    dtors: RefCell<Option<DtorChain<'a>>>
}

struct DtorChain<'a> {
    dtor: Box<FnBox() + 'a>,
    next: Option<Box<DtorChain<'a>>>
}

enum JoinState {
    Running(thread::JoinHandle<()>),
    Joined(thread::Result<()>),
    Empty,
}

impl JoinState {
    fn join(&mut self) {
        let mut state = JoinState::Empty;
        mem::swap(self, &mut state);
        if let JoinState::Running(handle) = state {
            *self = JoinState::Joined(handle.join())
        }
    }

    fn join_and_extract(&mut self) -> thread::Result<()> {
        let mut state = JoinState::Empty;
        mem::swap(self, &mut state);
        match state {
            JoinState::Running(handle) => handle.join(),
            JoinState::Joined(res) => res,
            _ => unreachable!(),
        }
    }
}

pub struct ScopedJoinHandle<T> {
    inner: Rc<RefCell<JoinState>>,
    packet: Arc<AtomicOption<T>>,
    thread: thread::Thread,
}

pub fn scope<'a, F, R>(f: F) -> R where F: FnOnce(&Scope<'a>) -> R {
    let mut scope = Scope { dtors: RefCell::new(None) };
    let ret = f(&scope);
    scope.drop_all();
    ret
}

impl<'a> Scope<'a> {
    // This method is carefully written in a transactional style, so
    // that it can be called directly and, if any dtor panics, can be
    // resumed in the unwinding this causes. By initially running the
    // method outside of any destructor, we avoid any leakage problems
    // due to #14875.
    fn drop_all(&mut self) {
        loop {
            // use a separate scope to ensure that the RefCell borrow
            // is relinquishe before running `dtor`
            let dtor = {
                let mut dtors = self.dtors.borrow_mut();
                if let Some(mut node) = dtors.take() {
                    *dtors = node.next.take().map(|b| *b);
                    node.dtor
                } else {
                    return
                }
            };
            dtor()
        }
    }

    pub fn defer<F>(&self, f: F) where F: FnOnce() + 'a {
        let mut dtors = self.dtors.borrow_mut();
        *dtors = Some(DtorChain {
            dtor: Box::new(f),
            next: dtors.take().map(Box::new)
        });
    }

    pub fn spawn<F, T>(&self, f: F) -> ScopedJoinHandle<T> where
        F: FnOnce() -> T + Send + 'a, T: Send + 'a
    {
        let their_packet = Arc::new(AtomicOption::new());
        let my_packet = their_packet.clone();

        let join_handle = unsafe {
            spawn_raw(move || {
                their_packet.swap(f(), Ordering::Relaxed);
            })
        };

        let thread = join_handle.thread().clone();
        let deferred_handle = Rc::new(RefCell::new(JoinState::Running(join_handle)));
        let my_handle = deferred_handle.clone();

        self.defer(move || {
            let mut state = deferred_handle.borrow_mut();
            state.join();
        });

        ScopedJoinHandle {
            inner: my_handle,
            packet: my_packet,
            thread: thread,
        }
    }
}

impl<T> ScopedJoinHandle<T> {
    pub fn join(self) -> T {
        self.inner.borrow_mut().join_and_extract().map(|_| {
            self.packet.take(Ordering::Relaxed).unwrap()
        }).unwrap()
    }

    pub fn thread(&self) -> &thread::Thread {
        &self.thread
    }
}

impl<'a> Drop for Scope<'a> {
    fn drop(&mut self) {
        self.drop_all()
    }
}
