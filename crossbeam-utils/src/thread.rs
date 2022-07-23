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
//! thread::scope(|s| {
//!     for person in &people {
//!         s.spawn(move |_| {
//!             println!("Hello, {}!", person);
//!         });
//!     }
//! });
//! ```
//!
//! # Why scoped threads?
//!
//! Suppose we wanted to re-write the previous example using plain threads:
//!
//! ```compile_fail,E0597
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
//! # Nesting scoped threads
//!
//! Sometimes scoped threads need to spawn more threads within the same scope. This is a little
//! tricky because argument `s` lives *inside* the invocation of `thread::scope()` and as such
//! cannot be borrowed by scoped threads:
//!
//! ```compile_fail,E0373,E0521
//! use crossbeam_utils::thread;
//!
//! thread::scope(|s| {
//!     s.spawn(|_| {
//!         // Not going to compile because we're trying to borrow `s`,
//!         // which lives *inside* the scope! :(
//!         s.spawn(|_| println!("nested thread"));
//!     });
//! });
//! ```
//!
//! Fortunately, there is a solution. Every scoped thread is passed a reference to its scope as an
//! argument, which can be used for spawning nested threads:
//!
//! ```
//! use crossbeam_utils::thread;
//!
//! thread::scope(|s| {
//!     // Note the `|s|` here.
//!     s.spawn(|s| {
//!         // Yay, this works because we're using a fresh argument `s`! :)
//!         s.spawn(|_| println!("nested thread"));
//!     });
//! });
//! ```

use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::mem;
use std::panic;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::sync::WaitGroup;
use cfg_if::cfg_if;

/// Creates a new scope for spawning threads.
///
/// All child threads that haven't been manually joined will be automatically joined just before
/// this function invocation ends. If all joined threads have successfully completed, `Ok` is
/// returned with the return value of `f`. If any of the joined threads has panicked, an `Err` is
/// returned containing errors from panicked threads. Note that if panics are implemented by
/// aborting the process, no error is returned; see the notes of [std::panic::catch_unwind].
///
/// # Examples
///
/// ```
/// use crossbeam_utils::thread;
///
/// let var = vec![1, 2, 3];
///
/// thread::scope(|s| {
///     s.spawn(|_| {
///         println!("A child thread borrowing `var`: {:?}", var);
///     });
/// });
/// ```
pub fn scope<'env, F, R>(f: F) -> R
where
    F: FnOnce(&Scope<'env>) -> R,
{
    let wg = WaitGroup::new();
    let scope = Scope::<'env> {
        num_unhandled_panicked_threads: Arc::new(AtomicUsize::new(0)),
        wait_group: wg.clone(),
        _marker: PhantomData,
    };

    // Execute the scoped function, but catch any panics.
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| f(&scope)));

    // Wait until all nested scopes are dropped.
    drop(scope.wait_group);
    wg.wait();

    // If `f` has panicked, resume unwinding.
    // If any of the child threads have panicked, return the panic errors.
    // Otherwise, everything is OK and return the result of `f`.
    match result {
        Err(err) => panic::resume_unwind(err),
        Ok(res) => {
            let num_unhandled_panicked_threads =
                scope.num_unhandled_panicked_threads.load(Ordering::Acquire);
            if num_unhandled_panicked_threads != 0 {
                panic!(
                    "{} scoped thread(s) panicked",
                    num_unhandled_panicked_threads
                )
            }
            res
        }
    }
}

/// A scope for spawning threads.
pub struct Scope<'env> {
    /// Number of unhandled panicked threads.
    num_unhandled_panicked_threads: Arc<AtomicUsize>,

    /// Used to wait until all subscopes all dropped.
    wait_group: WaitGroup,

    /// Borrows data with invariant lifetime `'env`.
    _marker: PhantomData<&'env mut &'env ()>,
}

unsafe impl Sync for Scope<'_> {}

impl<'env> Scope<'env> {
    /// Spawns a scoped thread.
    ///
    /// This method is similar to the [`spawn`] function in Rust's standard library. The difference
    /// is that this thread is scoped, meaning it's guaranteed to terminate before the scope exits,
    /// allowing it to reference variables outside the scope.
    ///
    /// The scoped thread is passed a reference to this scope as an argument, which can be used for
    /// spawning nested threads.
    ///
    /// The returned [handle](ScopedJoinHandle) can be used to manually
    /// [join](ScopedJoinHandle::join) the thread before the scope exits.
    ///
    /// This will create a thread using default parameters of [`ScopedThreadBuilder`], if you want to specify the
    /// stack size or the name of the thread, use this API instead.
    ///
    /// [`spawn`]: std::thread::spawn
    ///
    /// # Panics
    ///
    /// Panics if the OS fails to create a thread; use [`ScopedThreadBuilder::spawn`]
    /// to recover from such errors.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    ///
    /// thread::scope(|s| {
    ///     let handle = s.spawn(|_| {
    ///         println!("A child thread is running");
    ///         42
    ///     });
    ///
    ///     // Join the thread and retrieve its result.
    ///     let res = handle.join().unwrap();
    ///     assert_eq!(res, 42);
    /// });
    /// ```
    pub fn spawn<'scope, F, T>(&'scope self, f: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce(&Scope<'env>) -> T,
        F: Send + 'env,
        T: Send + 'env,
    {
        self.builder()
            .spawn(f)
            .expect("failed to spawn scoped thread")
    }

    /// Creates a builder that can configure a thread before spawning.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    ///
    /// thread::scope(|s| {
    ///     s.builder()
    ///         .spawn(|_| println!("A child thread is running"))
    ///         .unwrap();
    /// });
    /// ```
    pub fn builder<'scope>(&'scope self) -> ScopedThreadBuilder<'scope, 'env> {
        ScopedThreadBuilder {
            scope: self,
            builder: thread::Builder::new(),
        }
    }
}

impl fmt::Debug for Scope<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Scope { .. }")
    }
}

/// Configures the properties of a new thread.
///
/// The two configurable properties are:
///
/// - [`name`]: Specifies an [associated name for the thread][naming-threads].
/// - [`stack_size`]: Specifies the [desired stack size for the thread][stack-size].
///
/// The [`spawn`] method will take ownership of the builder and return an [`io::Result`] of the
/// thread handle with the given configuration.
///
/// The [`Scope::spawn`] method uses a builder with default configuration and unwraps its return
/// value. You may want to use this builder when you want to recover from a failure to launch a
/// thread.
///
/// # Examples
///
/// ```
/// use crossbeam_utils::thread;
///
/// thread::scope(|s| {
///     s.builder()
///         .spawn(|_| println!("Running a child thread"))
///         .unwrap();
/// });
/// ```
///
/// [`name`]: ScopedThreadBuilder::name
/// [`stack_size`]: ScopedThreadBuilder::stack_size
/// [`spawn`]: ScopedThreadBuilder::spawn
/// [`io::Result`]: std::io::Result
/// [naming-threads]: std::thread#naming-threads
/// [stack-size]: std::thread#stack-size
#[must_use = "must eventually spawn the thread"]
#[derive(Debug)]
pub struct ScopedThreadBuilder<'scope, 'env> {
    scope: &'scope Scope<'env>,
    builder: thread::Builder,
}

impl<'scope, 'env> ScopedThreadBuilder<'scope, 'env> {
    /// Sets the name for the new thread.
    ///
    /// The name must not contain null bytes (`\0`).
    ///
    /// For more information about named threads, see [here][naming-threads].
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    /// use std::thread::current;
    ///
    /// thread::scope(|s| {
    ///     s.builder()
    ///         .name("my thread".to_string())
    ///         .spawn(|_| assert_eq!(current().name(), Some("my thread")))
    ///         .unwrap();
    /// });
    /// ```
    ///
    /// [naming-threads]: std::thread#naming-threads
    pub fn name(mut self, name: String) -> ScopedThreadBuilder<'scope, 'env> {
        self.builder = self.builder.name(name);
        self
    }

    /// Sets the size of the stack for the new thread.
    ///
    /// The stack size is measured in bytes.
    ///
    /// For more information about the stack size for threads, see [here][stack-size].
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    ///
    /// thread::scope(|s| {
    ///     s.builder()
    ///         .stack_size(32 * 1024)
    ///         .spawn(|_| println!("Running a child thread"))
    ///         .unwrap();
    /// });
    /// ```
    ///
    /// [stack-size]: std::thread#stack-size
    pub fn stack_size(mut self, size: usize) -> ScopedThreadBuilder<'scope, 'env> {
        self.builder = self.builder.stack_size(size);
        self
    }

    /// Spawns a scoped thread with this configuration.
    ///
    /// The scoped thread is passed a reference to this scope as an argument, which can be used for
    /// spawning nested threads.
    ///
    /// The returned handle can be used to manually join the thread before the scope exits.
    ///
    /// # Errors
    ///
    /// Unlike the [`Scope::spawn`] method, this method yields an
    /// [`io::Result`] to capture any failure to create the thread at
    /// the OS level.
    ///
    /// [`io::Result`]: std::io::Result
    ///
    /// # Panics
    ///
    /// Panics if a thread name was set and it contained null bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    ///
    /// thread::scope(|s| {
    ///     let handle = s.builder()
    ///         .spawn(|_| {
    ///             println!("A child thread is running");
    ///             42
    ///         })
    ///         .unwrap();
    ///
    ///     // Join the thread and retrieve its result.
    ///     let res = handle.join().unwrap();
    ///     assert_eq!(res, 42);
    /// });
    /// ```
    pub fn spawn<F, T>(self, f: F) -> io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce(&Scope<'env>) -> T,
        F: Send + 'env,
        T: Send + 'env,
    {
        // The result of `f` will be stored here.
        let result = Arc::new(Mutex::new(ResultState::None));

        let wait_group = self.scope.wait_group.clone();
        let num_unhandled_panicked_threads = Arc::clone(&self.scope.num_unhandled_panicked_threads);

        // Spawn the thread and grab its join handle and thread handle.
        let (handle, thread) = {
            let result = Arc::clone(&result);

            // A clone of the scope that will be moved into the new thread.
            let scope = Scope::<'env> {
                num_unhandled_panicked_threads: Arc::clone(&num_unhandled_panicked_threads),
                wait_group: self.scope.wait_group.clone(),
                _marker: PhantomData,
            };

            // Spawn the thread.
            let handle = {
                let closure = move || {
                    struct IncrementOnPanic<'env>(Scope<'env>);
                    impl Drop for IncrementOnPanic<'_> {
                        fn drop(&mut self) {
                            if thread::panicking() {
                                // Always increment the counter here. If panic is handled
                                // by `.join()`, the counter is decremented later.
                                self.0
                                    .num_unhandled_panicked_threads
                                    .fetch_add(1, Ordering::Release);
                            }
                        }
                    }

                    // Make sure the scope is inside the closure with the proper `'env` lifetime.
                    let scope: Scope<'env> = scope;
                    let scope = IncrementOnPanic(scope);

                    // Run the closure.
                    let res = f(&scope.0);

                    match &mut *result.lock().unwrap() {
                        // If the thread is detached, the result must be dropped
                        // before the WaitGroup is dropped.
                        ResultState::ShouldDrop => drop(res),
                        // Store the result if the closure didn't panic.
                        result => *result = ResultState::Some(res),
                    }

                    drop(scope);
                };

                // Allocate `closure` on the heap and erase the `'env` bound.
                let closure: Box<dyn FnOnce() + Send + 'env> = Box::new(closure);
                let closure: Box<dyn FnOnce() + Send + 'static> =
                    unsafe { mem::transmute(closure) };

                // Finally, spawn the closure.
                self.builder.spawn(closure)?
            };

            let thread = handle.thread().clone();
            (handle, thread)
        };

        Ok(ScopedJoinHandle {
            handle: Some(handle),
            num_unhandled_panicked_threads,
            result,
            thread,
            _wait_group: wait_group,
            _marker: PhantomData,
        })
    }
}

enum ResultState<T> {
    None,
    Some(T),
    ShouldDrop,
}

impl<T> ResultState<T> {
    fn take(&mut self) -> Option<T> {
        match mem::replace(self, ResultState::None) {
            ResultState::Some(v) => Some(v),
            _ => None,
        }
    }
}

unsafe impl<T> Send for ScopedJoinHandle<'_, T> {}
unsafe impl<T> Sync for ScopedJoinHandle<'_, T> {}

/// A handle that can be used to join its scoped thread.
///
/// This struct is created by the [`Scope::spawn`] method and the
/// [`ScopedThreadBuilder::spawn`] method.
pub struct ScopedJoinHandle<'scope, T> {
    /// A join handle to the spawned thread.
    handle: Option<thread::JoinHandle<()>>,

    /// Number of unhandled panicked threads.
    num_unhandled_panicked_threads: Arc<AtomicUsize>,

    /// Holds the result of the inner closure.
    result: Arc<Mutex<ResultState<T>>>,

    /// A handle to the the spawned thread.
    thread: thread::Thread,

    _wait_group: WaitGroup,

    /// Borrows the parent scope with lifetime `'scope`.
    _marker: PhantomData<&'scope ()>,
}

impl<T> ScopedJoinHandle<'_, T> {
    /// Waits for the thread to finish and returns its result.
    ///
    /// If the child thread panics, an error is returned. Note that if panics are implemented by
    /// aborting the process, no error is returned; see the notes of [std::panic::catch_unwind].
    ///
    /// # Panics
    ///
    /// This function may panic on some platforms if a thread attempts to join itself or otherwise
    /// may create a deadlock with joining threads.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    ///
    /// thread::scope(|s| {
    ///     let handle1 = s.spawn(|_| println!("I'm a happy thread :)"));
    ///     let handle2 = s.spawn(|_| panic!("I'm a sad thread :("));
    ///
    ///     // Join the first thread and verify that it succeeded.
    ///     let res = handle1.join();
    ///     assert!(res.is_ok());
    ///
    ///     // Join the second thread and verify that it panicked.
    ///     let res = handle2.join();
    ///     assert!(res.is_err());
    /// });
    /// ```
    pub fn join(mut self) -> thread::Result<T> {
        // Take out the handle. The handle will surely be available because the root scope waits
        // for nested scopes before joining remaining threads.
        let handle = self.handle.take().unwrap();

        // Join the thread and then take the result out of its inner closure.
        match handle.join() {
            Ok(()) => Ok(self.result.lock().unwrap().take().unwrap()),
            Err(e) => {
                // Decrement the counter as we handled a panic from this handle.
                self.num_unhandled_panicked_threads
                    .fetch_sub(1, Ordering::Release);
                Err(e)
            }
        }
    }

    /// Returns a handle to the underlying thread.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::thread;
    ///
    /// thread::scope(|s| {
    ///     let handle = s.spawn(|_| println!("A child thread is running"));
    ///     println!("The child thread ID: {:?}", handle.thread().id());
    /// });
    /// ```
    pub fn thread(&self) -> &thread::Thread {
        &self.thread
    }
}

impl<T> Drop for ScopedJoinHandle<'_, T> {
    fn drop(&mut self) {
        *self
            .result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ResultState::ShouldDrop;
    }
}

cfg_if! {
    if #[cfg(unix)] {
        use std::os::unix::thread::{JoinHandleExt, RawPthread};

        // TODO: Use own unix::JoinHandleExt trait https://github.com/crossbeam-rs/crossbeam/issues/651
        impl<T> JoinHandleExt for ScopedJoinHandle<'_, T> {
            fn as_pthread_t(&self) -> RawPthread {
                // Borrow the handle. The handle will surely be available because the root scope waits
                // for nested scopes before joining remaining threads.
                self.handle.as_ref().unwrap().as_pthread_t()
            }
            fn into_pthread_t(self) -> RawPthread {
                self.as_pthread_t()
            }
        }
    } else if #[cfg(windows)] {
        use std::os::windows::io::{AsRawHandle, IntoRawHandle, RawHandle};

        impl<T> AsRawHandle for ScopedJoinHandle<'_, T> {
            fn as_raw_handle(&self) -> RawHandle {
                // Borrow the handle. The handle will surely be available because the root scope waits
                // for nested scopes before joining remaining threads.
                self.handle.as_ref().unwrap().as_raw_handle()
            }
        }

        impl<T> IntoRawHandle for ScopedJoinHandle<'_, T> {
            fn into_raw_handle(self) -> RawHandle {
                self.as_raw_handle()
            }
        }
    }
}

impl<T> fmt::Debug for ScopedJoinHandle<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("ScopedJoinHandle { .. }")
    }
}
