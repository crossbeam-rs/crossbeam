//! Threads that can borrow variables from the stack.
//!
//! Create a scope when spawned threads need to access variables on the stack:
//!
//! ```
//! use crossbeam_utils::thread;
//!
//! let people = vec![
//!     "Alice".to_string(),
//!     "Bob".to_string(),
//!     "Carol".to_string(),
//! ];
//!
//! thread::scope(|scope| {
//!     for person in &people {
//!         scope.spawn(move || {
//!             println!("Hello, {}!", person);
//!         });
//!     }
//! }).unwrap();
//! ```
//!
//! # Why scoped threads?
//!
//! Suppose we wanted to re-write the previous example using plain threads:
//!
//! ```ignore
//! use std::thread;
//!
//! let people = vec![
//!     "Alice".to_string(),
//!     "Bob".to_string(),
//!     "Carol".to_string(),
//! ];
//!
//! let mut threads = Vec::new();
//!
//! for person in &people {
//!     threads.push(thread::spawn(move || {
//!         println!("Hello, {}!", person);
//!     }));
//! }
//!
//! for thread in threads {
//!     thread.join().unwrap();
//! }
//! ```
//!
//! This doesn't work because the borrow checker complains about `people` not living long enough:
//!
//! ```text
//! error[E0597]: `people` does not live long enough
//!   --> src/main.rs:12:20
//!    |
//! 12 |     for person in &people {
//!    |                    ^^^^^^ borrowed value does not live long enough
//! ...
//! 21 | }
//!    | - borrowed value only lives until here
//!    |
//!    = note: borrowed value must be valid for the static lifetime...
//! ```
//!
//! The problem here is that spawned threads are not allowed to borrow variables on stack because
//! the compiler cannot prove they will be joined before `people` is destroyed.
//!
//! Scoped threads are a mechanism to guarantee to the compiler that spawned threads will be joined
//! before the scope ends.
//!
//! # How scoped threads work
//!
//! If a variable is borrowed by a thread, the thread must complete before the variable is
//! destroyed. Threads spawned using [`std::thread::spawn`] can only borrow variables with the
//! `'static` lifetime because the borrow checker cannot be sure when the thread will complete.
//!
//! A scope creates a clear boundary between variables outside the scope and threads inside the
//! scope. Whenever a scope spawns a thread, it promises to join the thread before the scope ends.
//! This way we guarantee to the borrow checker that scoped threads only live within the scope and
//! can safely access variables outside it.
//!
//! [`std::thread::spawn`]: https://doc.rust-lang.org/std/thread/fn.spawn.html

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::mem;
use std::panic;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

/// Like [`std::thread::spawn`], but without lifetime bounds on the closure.
///
/// [`std::thread::spawn`]: https://doc.rust-lang.org/stable/std/thread/fn.spawn.html
pub unsafe fn spawn_unchecked<'env, F, T>(f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T,
    F: Send + 'env,
    T: Send + 'static,
{
    let builder = thread::Builder::new();
    builder_spawn_unchecked(builder, f).unwrap()
}

/// Like [`std::thread::Builder::spawn`], but without lifetime bounds on the closure.
///
/// [`std::thread::Builder::spawn`]:
///     https://doc.rust-lang.org/nightly/std/thread/struct.Builder.html#method.spawn
pub unsafe fn builder_spawn_unchecked<'env, F, T>(
    builder: thread::Builder,
    f: F,
) -> io::Result<thread::JoinHandle<T>>
where
    F: FnOnce() -> T,
    F: Send + 'env,
    T: Send + 'static,
{
    let closure: Box<FnBox<T> + Send + 'env> = Box::new(f);
    let closure: Box<FnBox<T> + Send + 'static> = mem::transmute(closure);
    builder.spawn(move || closure.call_box())
}

/// Creates a new `Scope` for [*scoped thread spawning*](struct.Scope.html#method.spawn).
///
/// No matter what happens, before the `Scope` is dropped, it is guaranteed that all the unjoined
/// spawned scoped threads are joined.
///
/// `thread::scope()` returns `Ok(())` if all the unjoined spawned threads did not panic. It returns
/// `Err(e)` if one of them panics with `e`. If many of them panic, it is still guaranteed that all
/// the threads are joined, and `thread::scope()` returns `Err(e)` with `e` from a panicking thread.
///
/// # Examples
///
/// Creating and using a scope:
///
/// ```
/// crossbeam_utils::thread::scope(|scope| {
///     scope.spawn(|| println!("Exiting scope"));
///     scope.spawn(|| println!("Running child thread in scope"));
/// }).unwrap();
/// ```
pub fn scope<'env, F, R>(f: F) -> thread::Result<R>
where
    F: FnOnce(&Scope<'env>) -> R,
{
    let scope = Scope {
        joins: RefCell::new(Vec::new()),
        _marker: PhantomData,
    };

    // Execute the scoped function, but catch any panics.
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| f(&scope)));
    // Join all remaining spawned threads.
    let mut panics = scope.join_all();

    if panics.is_empty() {
        result.map_err(|res| Box::new(vec![res]) as _)
    } else {
        if let Err(err) = result {
            panics.reserve(1);
            panics.insert(0, err);
        }

        Err(Box::new(panics))
    }
}

pub struct Scope<'env> {
    /// The list of the thread join jobs.
    joins: RefCell<Vec<Box<FnBox<thread::Result<()>> + 'env>>>,
    // !Send + !Sync
    _marker: PhantomData<*const ()>,
}

impl<'env> Scope<'env> {
    /// Create a scoped thread.
    ///
    /// `spawn` is similar to the [`spawn`] function in Rust's standard library. The difference is
    /// that this thread is scoped, meaning that it's guaranteed to terminate before the current
    /// stack frame goes away, allowing you to reference the parent stack frame directly. This is
    /// ensured by having the parent thread join on the child thread before the scope exits.
    ///
    /// [`spawn`]: https://doc.rust-lang.org/std/thread/fn.spawn.html
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

    /// Join all remaining threads and return all potential error payloads
    fn join_all(self) -> Vec<Box<Any + Send + 'static>> {
        self
            .joins
            .into_inner()
            .into_iter()
            .filter_map(|join| join.call_box().err())
            .collect()
    }
}

impl<'env> fmt::Debug for Scope<'env> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Scope {{ ... }}")
    }
}

/// Scoped thread configuration. Provides detailed control over the properties and behavior of new
/// scoped threads.
pub struct ScopedThreadBuilder<'scope, 'env: 'scope> {
    scope: &'scope Scope<'env>,
    builder: thread::Builder,
}

impl<'scope, 'env> ScopedThreadBuilder<'scope, 'env> {
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

        let handle = ScopedJoinHandle {
            inner: Arc::new(Mutex::new(Some(join_state))),
            thread,
            _marker: PhantomData,
        };

        let deferred_handle = Arc::clone(&handle.inner);
        self.scope.joins.borrow_mut().push(Box::new(move || {
            let state = deferred_handle.lock().unwrap().take();
            if let Some(state) = state {
                state.join().map(|_| ())
            } else {
                Ok(())
            }
        }));

        Ok(handle)
    }
}

unsafe impl<'scope, T> Send for ScopedJoinHandle<'scope, T> {}
unsafe impl<'scope, T> Sync for ScopedJoinHandle<'scope, T> {}

/// A handle to a scoped thread
pub struct ScopedJoinHandle<'scope, T: 'scope> {
    inner: Arc<Mutex<Option<JoinState<T>>>>,
    thread: thread::Thread,
    _marker: PhantomData<&'scope T>,
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
        let state = self.inner.lock().unwrap().take();
        state.unwrap().join()
    }

    /// Gets the underlying [`std::thread::Thread`] handle.
    ///
    /// [`std::thread::Thread`]: https://doc.rust-lang.org/std/thread/struct.Thread.html
    pub fn thread(&self) -> &thread::Thread {
        &self.thread
    }
}

impl<'scope, T> fmt::Debug for ScopedJoinHandle<'scope, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ScopedJoinHandle {{ ... }}")
    }
}

type ScopedThreadResult<T> = Arc<Mutex<Option<T>>>;

struct JoinState<T> {
    join_handle: thread::JoinHandle<()>,
    result: ScopedThreadResult<T>,
}

impl<T> JoinState<T> {
    fn new(join_handle: thread::JoinHandle<()>, result: ScopedThreadResult<T>) -> JoinState<T> {
        JoinState {
            join_handle,
            result,
        }
    }

    fn join(self) -> thread::Result<T> {
        let result = self.result;
        self.join_handle
            .join()
            .map(|_| result.lock().unwrap().take().unwrap())
    }
}

trait FnBox<T> {
    fn call_box(self: Box<Self>) -> T;
}

impl<T, F: FnOnce() -> T> FnBox<T> for F {
    fn call_box(self: Box<Self>) -> T {
        (*self)()
    }
}

/// Returns a `usize` that identifies the current thread.
///
/// Each thread is associated with an 'index'. While there are no particular guarantees, indices
/// usually tend to be consecutive numbers between 0 and the number of running threads.
///
/// Since this function accesses TLS, `None` might be returned if the current thread's TLS is
/// tearing down.
#[inline]
pub fn current_index() -> Option<usize> {
    REGISTRATION.try_with(|reg| reg.index).ok()
}

/// The global registry keeping track of registered threads and indices.
struct ThreadIndices {
    /// Mapping from `ThreadId` to thread index.
    mapping: HashMap<ThreadId, usize>,

    /// A list of free indices.
    free_list: Vec<usize>,

    /// The next index to allocate if the free list is empty.
    next_index: usize,
}

lazy_static! {
    static ref THREAD_INDICES: Mutex<ThreadIndices> = Mutex::new(ThreadIndices {
        mapping: HashMap::new(),
        free_list: Vec::new(),
        next_index: 0,
    });
}

/// A registration of a thread with an index.
///
/// When dropped, unregisters the thread and frees the reserved index.
struct Registration {
    index: usize,
    thread_id: ThreadId,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut indices = THREAD_INDICES.lock().unwrap();
        indices.mapping.remove(&self.thread_id);
        indices.free_list.push(self.index);
    }
}

thread_local! {
    static REGISTRATION: Registration = {
        let thread_id = thread::current().id();
        let mut indices = THREAD_INDICES.lock().unwrap();

        let index = match indices.free_list.pop() {
            Some(i) => i,
            None => {
                let i = indices.next_index;
                indices.next_index += 1;
                i
            }
        };
        indices.mapping.insert(thread_id, index);

        Registration {
            index,
            thread_id,
        }
    };
}
