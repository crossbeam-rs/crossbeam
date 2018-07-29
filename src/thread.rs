/// Scoped thread.
///
/// # Examples
///
/// A basic scoped thread:
///
/// ```
/// crossbeam_utils::thread::scope(|scope| {
///     scope.spawn(|| {
///         println!("Hello from a scoped thread!");
///     });
/// }).unwrap();
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
/// `'static` lifetime. One way of giving it a proper lifetime is to use an [`Arc`][arc]:
///
/// [arc]: http://doc.rust-lang.org/stable/std/sync/struct.Arc.html
/// [spawn]: https://doc.rust-lang.org/stable/std/thread/fn.spawn.html
///
/// ```
/// use std::sync::Arc;
///
/// let array = Arc::new([1, 2, 3]);
/// let mut guards = vec![];
///
/// for i in 0..array.len() {
///     let a = array.clone();
///
///     let guard = std::thread::spawn(move || {
///         println!("element: {}", a[i]);
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
/// But this introduces unnecessary allocation, as `Arc<T>` puts its data on the heap, and we
/// also end up dealing with reference counts. We know that we're joining the threads before
/// our function returns, so just taking a reference _should_ be safe. Rust can't know that,
/// though.
///
/// Enter scoped threads. Here's our original example, using `spawn` from crossbeam rather
/// than from `std::thread`:
///
/// ```
/// let array = [1, 2, 3];
///
/// crossbeam_utils::thread::scope(|scope| {
///     for i in &array {
///         scope.spawn(move || {
///             println!("element: {}", i);
///         });
///     }
/// }).unwrap();
/// ```
///
/// Much more straightforward.

use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ops::DerefMut;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::thread;
use std::io;
use std::sync::{Arc, Mutex};

#[doc(hidden)]
trait FnBox<T> {
    fn call_box(self: Box<Self>) -> T;
}

impl<T, F: FnOnce() -> T> FnBox<T> for F {
    fn call_box(self: Box<Self>) -> T {
        (*self)()
    }
}

/// Like `std::thread::spawn`, but without lifetime bounds on the closure.
pub unsafe fn spawn_unchecked<'a, F, T>(f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T,
    F: Send + 'a,
    T: Send + 'static,
{
    let builder = thread::Builder::new();
    builder_spawn_unchecked(builder, f).unwrap()
}

/// Like `std::thread::Builder::spawn`, but without lifetime bounds on the closure.
pub unsafe fn builder_spawn_unchecked<'a, F, T>(
    builder: thread::Builder,
    f: F,
) -> io::Result<thread::JoinHandle<T>>
where
    F: FnOnce() -> T,
    F: Send + 'a,
    T: Send + 'static,
{
    let closure: Box<FnBox<T> + 'a> = Box::new(f);
    let closure: Box<FnBox<T> + Send> = mem::transmute(closure);
    builder.spawn(move || {
        closure.call_box()
    })
}

pub struct Scope<'env> {
    /// The list of the deferred functions and thread join jobs.
    dtors: RefCell<Option<DtorChain<'env, thread::Result<()>>>>,
    // !Send + !Sync
    _marker: PhantomData<*const ()>,
}

struct DtorChain<'env, T> {
    dtor: Box<FnBox<T> + 'env>,
    next: Option<Box<DtorChain<'env, T>>>,
}

impl<'env, T> DtorChain<'env, T> {
    pub fn pop(chain: &mut Option<DtorChain<'env, T>>) -> Option<Box<FnBox<T> + 'env>> {
        chain.take().map(|mut node| {
            *chain = node.next.take().map(|b| *b);
            node.dtor
        })
    }
}

struct JoinState<T> {
    join_handle: thread::JoinHandle<()>,
    result: ScopedThreadResult<T>
}

impl<T> JoinState<T> {
    fn new(join_handle: thread::JoinHandle<()>, result: ScopedThreadResult<T>) -> JoinState<T> {
        JoinState {
            join_handle,
            result
        }
    }

    fn join(self) -> thread::Result<T> {
        let result = self.result;
        self.join_handle.join().map(|_| {
            result.lock().unwrap().take().unwrap()
        })
    }
}

/// A handle to a scoped thread
pub struct ScopedJoinHandle<'scope, T: 'scope> {
    // !Send + !Sync
    inner: Rc<RefCell<Option<JoinState<T>>>>,
    thread: thread::Thread,
    _marker: PhantomData<&'scope T>,
}

unsafe impl<'scope, T> Send for ScopedJoinHandle<'scope, T> {}
unsafe impl<'scope, T> Sync for ScopedJoinHandle<'scope, T> {}

/// Create a new `Scope` for [*scoped thread spawning*](struct.Scope.html#method.spawn).
///
/// In addition, you can [register ad-hoc functions](struct.Scope.html#method.defer) that are
/// deferred to be run. No matter what happens, before the `Scope` is dropped, it is guaranteed that
/// all the unjoined spawned scoped threads are joined and the deferred functions are run.
///
/// `thread::scope()` returns `Ok(())` if all the unjoined spawned threads and the deferred
/// functions did not panic. It returns `Err(e)` if one of them panics with `e`. If many of them
/// panics, it is still guaranteed that all the threads are joined and all the functions are run,
/// and `thread::scope()` returns `Err(e)` with `e` from a panicking thread or function.
///
/// # Examples
///
/// Creating and using a scope:
///
/// ```
/// crossbeam_utils::thread::scope(|scope| {
///     scope.defer(|| println!("Exiting scope"));
///     scope.spawn(|| println!("Running child thread in scope"));
/// }).unwrap();
/// // Prints messages
/// ```
pub fn scope<'env, F, R>(f: F) -> thread::Result<R>
where
    F: FnOnce(&Scope<'env>) -> R,
{
    let mut scope = Scope {
        dtors: RefCell::new(None),
        _marker: PhantomData,
    };
    let ret = f(&scope);
    scope.drop_all()?;
    Ok(ret)
}

impl<'env> fmt::Debug for Scope<'env> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Scope {{ ... }}")
    }
}

impl<'scope, T> fmt::Debug for ScopedJoinHandle<'scope, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ScopedJoinHandle {{ ... }}")
    }
}

impl<'env> Scope<'env> {
    // This method is carefully written in a transactional style, so that it can be called directly
    // and, if any dtor panics, can be resumed in the unwinding this causes. By initially running
    // the method outside of any destructor, we avoid any leakage problems due to
    // @rust-lang/rust#14875.
    fn drop_all(&mut self) -> thread::Result<()> {
        let mut ret = Ok(());
        while let Some(dtor) = DtorChain::pop(&mut self.dtors.borrow_mut()) {
            ret = ret.and(dtor.call_box());
        }
        ret
    }

    fn defer_inner<F>(&self, f: F)
    where
        F: (FnOnce() -> thread::Result<()>) + 'env,
    {
        let mut dtors = self.dtors.borrow_mut();
        *dtors = Some(DtorChain {
            dtor: Box::new(f),
            next: dtors.take().map(Box::new),
        });
    }

    /// Schedule code to be executed when exiting the scope.
    ///
    /// This is akin to having a destructor on the stack, except that it is *guaranteed* to be
    /// run. It is guaranteed that the function is called after all the spawned threads are joined.
    pub fn defer<F>(&self, f: F)
    where
        F: FnOnce() + 'env,
    {
        self.defer_inner(move || panic::catch_unwind(AssertUnwindSafe(f)));
    }

    /// Create a scoped thread.
    ///
    /// `spawn` is similar to the [`spawn`][spawn] function in Rust's standard library. The
    /// difference is that this thread is scoped, meaning that it's guaranteed to terminate before
    /// the current stack frame goes away, allowing you to reference the parent stack frame
    /// directly. This is ensured by having the parent thread join on the child thread before the
    /// scope exits.
    ///
    /// [spawn]: http://doc.rust-lang.org/std/thread/fn.spawn.html
    pub fn spawn<'scope, F, T>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T,
        F: Send + 'env,
        T: Send + 'env,
    {
        self.builder().spawn(f).unwrap()
    }

    /// Generates the base configuration for spawning a scoped thread, from which configuration
    /// methods can be chained.
    pub fn builder<'scope>(&'scope self) -> ScopedThreadBuilder<'scope, 'env> {
        ScopedThreadBuilder {
            scope: self,
            builder: thread::Builder::new(),
        }
    }
}

/// Scoped thread configuration. Provides detailed control over the properties and behavior of new
/// scoped threads.
pub struct ScopedThreadBuilder<'scope, 'env: 'scope> {
    scope: &'scope Scope<'env>,
    builder: thread::Builder,
}

impl<'scope, 'env: 'scope> ScopedThreadBuilder<'scope, 'env> {
    /// Names the thread-to-be. Currently the name is used for identification only in panic
    /// messages.
    pub fn name(mut self, name: String) -> ScopedThreadBuilder<'scope, 'env> {
        self.builder = self.builder.name(name);
        self
    }

    /// Sets the size of the stack for the new thread.
    pub fn stack_size(mut self, size: usize) -> ScopedThreadBuilder<'scope, 'env> {
        self.builder = self.builder.stack_size(size);
        self
    }

    /// Spawns a new thread, and returns a join handle for it.
    pub fn spawn<F, T>(self, f: F) -> io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce() -> T,
        F: Send + 'env,
        T: Send + 'env,
    {
        let result = Arc::new(Mutex::new(None));

        let join_handle = unsafe {
            let mut thread_result = Arc::clone(&result);
            builder_spawn_unchecked(self.builder, move || {
                *thread_result.lock().unwrap() = Some(f());
            })
        }?;

        let thread = join_handle.thread().clone();

        let join_state = JoinState::<T>::new(join_handle, result);
        let deferred_handle = Rc::new(RefCell::new(Some(join_state)));
        let my_handle = deferred_handle.clone();

        self.scope.defer_inner(move || {
            let state = deferred_handle.borrow_mut().deref_mut().take();
            if let Some(state) = state {
                state.join().map(|_| ())
            } else {
                Ok(())
            }
        });

        Ok(ScopedJoinHandle {
            inner: my_handle,
            thread: thread,
            _marker: PhantomData,
        })
    }
}

impl<'scope, T> ScopedJoinHandle<'scope, T> {
    /// Waits for the associated thread to finish.
    ///
    /// If the child thread panics, [`Err`] is returned with the parameter given to [`panic`].
    ///
    /// [`Err`]: https://doc.rust-lang.org/std/result/enum.Result.html#variant.Err
    /// [`panic`]: https://doc.rust-lang.org/std/macro.panic.html
    ///
    /// # Panics
    ///
    /// This function may panic on some platforms if a thread attempts to join itself or otherwise
    /// may create a deadlock with joining threads.
    pub fn join(self) -> thread::Result<T> {
        let state = self.inner.borrow_mut().deref_mut().take();
        state.unwrap().join()
    }

    /// Gets the underlying [`std::thread::Thread`] handle.
    ///
    /// [`std::thread::Thread`]: https://doc.rust-lang.org/std/thread/struct.Thread.html
    pub fn thread(&self) -> &thread::Thread {
        &self.thread
    }
}

impl<'env> Drop for Scope<'env> {
    fn drop(&mut self) {
        // Actually, there should be no deferred functions left to be run.
        self.drop_all().unwrap();
    }
}

type ScopedThreadResult<T> = Arc<Mutex<Option<T>>>;
