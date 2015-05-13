#![feature(unique)]
#![feature(core)]
#![feature(catch_panic)]

use std::rc::Rc;
use std::cell::RefCell;
use std::boxed::FnBox;
use std::any::Any;
use std::mem;
use std::thread;
use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc;
use std::ptr::{self, Unique};

use atomic_option::AtomicOption;

mod atomic_option;
mod raw_thread;

pub type WorkResult<T> = Result<T, Box<Any + Send>>;

pub struct WorkHandle<T> {
    rx: mpsc::Receiver<WorkResult<T>>,
}

impl<T> WorkHandle<T> {
    pub fn join(self) -> WorkResult<T> {
        self.rx.recv().unwrap()
    }
}

struct WorkItem {
    closure: Box<FnBox() + Send>,
    on_failure: Box<FnBox(Box<Any + Send>) + Send>,
}

struct WorkQueue {
    inner: Vec<WorkItem>,
    shutdown: bool,
}

pub struct ThreadPool {
    queue: Arc<Mutex<WorkQueue>>,
    condvar: Arc<Condvar>,
}

struct Worker {
    queue: Arc<Mutex<WorkQueue>>,
    condvar: Arc<Condvar>,
}

impl Worker {
    fn work(&self) {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(work) = queue.inner.pop() {
                let WorkItem { closure, on_failure } = work;
                if let Err(err) = thread::catch_panic(move || closure.call_box(())) {
                    on_failure.call_box((err,));
                }
            } else if queue.shutdown {
                break;
            } else {
                queue = self.condvar.wait(queue).unwrap();
            }
        }
    }
}

impl ThreadPool {
    pub fn new(count: u32) -> ThreadPool {
        let mutex = Arc::new(Mutex::new(WorkQueue {
            inner: Vec::new(),
            shutdown: false,
        }));
        let condvar = Arc::new(Condvar::new());

        for _ in 0..count {
            let worker = Worker {
                queue: mutex.clone(),
                condvar: condvar.clone(),
            };
            thread::spawn(move || worker.work());
        }

        ThreadPool {
            queue: mutex,
            condvar: condvar,
        }
    }

    fn push_raw<'a, F, T>(&self, f: F) -> WorkHandle<T> where
        F: FnOnce() -> T + 'a + Send,
        T: Send + 'a
    {
        let (tx_ok, rx) = mpsc::channel();
        let tx_err = tx_ok.clone();

        let closure: Box<FnOnce() + Send + 'a> = Box::new(move || {
            tx_ok.send(Ok(f())).unwrap();
        });
        let on_failure: Box<FnOnce(Box<Any + Send>) + Send + 'a> = Box::new(move |err| {
            tx_err.send(Err(err)).unwrap();
        });

        self.queue.lock().unwrap().inner.push(WorkItem {
            closure: unsafe { mem::transmute(closure) },
            on_failure: unsafe { mem::transmute(on_failure) },
        });
        self.condvar.notify_one();

        WorkHandle { rx: rx }
    }

    pub fn push_work<F, T>(&self, f: F) -> WorkHandle<T> where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static
    {
        self.push_raw(f)
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.queue.lock().unwrap().shutdown = true;
    }
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
    Joined(WorkResult<()>),
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

    fn join_and_extract(&mut self) -> WorkResult<()> {
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
            raw_thread::spawn(move || {
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
    pub fn join(self) -> WorkResult<T> {
        self.inner.borrow_mut().join_and_extract().map(|_| {
            self.packet.take(Ordering::Relaxed).unwrap()
        })
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
