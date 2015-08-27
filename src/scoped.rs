use std::cell::RefCell;
use std::mem;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

use {spawn_unsafe, FnBox};
use sync::AtomicOption;

pub struct Scope<'a> {
    dtors: RefCell<Option<DtorChain<'a>>>
}

struct DtorChain<'a> {
    dtor: Box<FnBox + 'a>,
    next: Option<Box<DtorChain<'a>>>
}

enum JoinState {
    Running(thread::JoinHandle<()>),
    Joined,
}

impl JoinState {
    fn join(&mut self) {
        let mut state = JoinState::Joined;
        mem::swap(self, &mut state);
        if let JoinState::Running(handle) = state {
            let res = handle.join();

            if !thread::panicking() { res.unwrap(); }
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
            dtor.call_box()
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
            spawn_unsafe(move || {
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
        self.inner.borrow_mut().join();
        self.packet.take(Ordering::Relaxed).unwrap()
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
