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

/// Create a new `scope`, for scoped concurrency operations.
///
/// Using crossbeam for concurrency involves creating a [`Scope`][scope]. That's the job of
/// `scope`.
///
/// [scope]: struct.Scope.html
///
/// # Examples
///
/// Creating a scope:
///
/// ```
/// crossbeam::scope(|scope| {
///     // do stuff with scope here
/// });
/// ```
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
    // due to @rust-lang/rust#14875.
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

    /// Create a scoped thread.
    ///
    /// `spawn` is similar to the [`spawn`][spawn] function in Rust's standard library. The
    /// difference is that this thread is scoped, meaning that it's guaranteed to terminate
    /// before the current stack frame goes away, allowing you to reference the parent stack frame
    /// directly. This is ensured by having the parent thread join on the child thread before the
    /// scope exits.
    ///
    /// [spawn]: http://doc.rust-lang.org/std/thread/fn.spawn.html
    ///
    /// # Examples
    ///
    /// A basic scoped thread:
    ///
    /// ```
    /// crossbeam::scope(|scope| {
    ///     scope.spawn(|| {
    ///         println!("Hello from a scoped thread!");
    ///     });
    /// });
    /// ```
    ///
    /// When writing concurrent Rust programs, you'll sometimes see a pattern like this, using
    /// [`std::thread::spawn`][spawn]:
    ///
    /// ```ignore
    /// let array = [1, 2, 3];
    /// let mut guards = vec![];
    ///
    /// for i in &array {
    ///     let guard = std::thread::spawn(move || {
    ///         println!("element: {}", i);
    ///     });
    ///
    ///     guards.push(guard);
    /// }
    ///
    /// for guard in guards {
    ///     guard.join().unwrap();
    /// }
    /// ```
    ///
    /// The basic pattern is:
    ///
    /// 1. Iterate over some collection.
    /// 2. Spin up a thread to operate on each part of the collection.
    /// 3. Join all the threads.
    ///
    /// However, this code actually gives an error:
    ///
    /// ```text
    /// error: `array` does not live long enough
    /// for i in &array {
    ///           ^~~~~
    /// in expansion of for loop expansion
    /// note: expansion site
    /// note: reference must be valid for the static lifetime...
    /// note: ...but borrowed value is only valid for the block suffix following statement 0 at ...
    ///     let array = [1, 2, 3];
    ///     let mut guards = vec![];
    ///     
    ///     for i in &array {
    ///         let guard = std::thread::spawn(move || {
    ///             println!("element: {}", i);
    /// ...
    /// error: aborting due to previous error
    /// ```
    ///
    /// Because [`std::thread::spawn`][spawn] doesn't know about this scope, it requires a
    /// `'static` lifetime. One way of giving it a proper lifetime is to [`Box`][box] it up:
    ///
    /// [box]: http://doc.rust-lang.org/std/boxed/struct.Box.html
    ///
    /// ```
    /// let array = Box::new([1, 2, 3]);
    /// let mut guards = vec![];
    ///
    /// for i in (0..array.len()) {
    ///     let i = array[i];
    ///     let guard = std::thread::spawn(move || {
    ///         println!("element: {}", i);
    ///     });
    ///
    ///     guards.push(guard);
    /// }
    ///
    /// for guard in guards {
    ///     guard.join().unwrap();
    /// }
    /// ```
    ///
    /// But this introduces unnecessary allocation: we know that we're joining the threads before
    /// our function returns, so just taking a reference _should_ be safe. Rust can't know that,
    /// though.
    ///
    /// Enter scoped threads. Here's our original example, using `spawn` from crossbeam rather
    /// than from `std::thread`:
    ///
    /// ```
    /// let array = [1, 2, 3];
    ///
    /// crossbeam::scope(|scope| {
    ///     for i in &array {
    ///         scope.spawn(move || {
    ///             println!("element: {}", i);
    ///         });
    ///     }
    /// });
    /// ```
    ///
    /// Much more straightforward.
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
