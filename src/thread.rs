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
/// [`std::thread::spawn`]:
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
/// Because [`std::thread::spawn`] doesn't know about this scope, it requires a `'static` lifetime.
/// One way of giving it a proper lifetime is to use an [`Arc`]:
///
/// [`Arc`]: https://doc.rust-lang.org/stable/std/sync/struct.Arc.html
/// [`std::thread::spawn`]: https://doc.rust-lang.org/stable/std/thread/fn.spawn.html
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
use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::io;
use std::marker::PhantomData;
use std::mem;
use std::ops::DerefMut;
use std::panic;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

#[doc(hidden)]
trait FnBox<T> {
    fn call_box(self: Box<Self>) -> T;
}

impl<T, F: FnOnce() -> T> FnBox<T> for F {
    fn call_box(self: Box<Self>) -> T {
        (*self)()
    }
}

/// Like [`std::thread::spawn`], but without lifetime bounds on the closure.
///
/// [`std::thread::spawn`]: https://doc.rust-lang.org/stable/std/thread/fn.spawn.html
pub unsafe fn spawn_unchecked<'a, F, T>(f: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T,
    F: Send + 'a,
    T: Send + 'static,
{
    let builder = thread::Builder::new();
    builder_spawn_unchecked(builder, f).unwrap()
}

/// Like [`std::thread::Builder::spawn`], but without lifetime bounds on the closure.
///
/// [`std::thread::Builder::spawn`]:
///     https://doc.rust-lang.org/nightly/std/thread/struct.Builder.html#method.spawn
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
    builder.spawn(move || closure.call_box())
}

pub struct Scope<'env> {
    /// The list of the thread join jobs.
    joins: RefCell<Vec<Box<FnBox<thread::Result<()>> + 'env>>>,
    /// Thread panics invoked so far.
    panics: Vec<Box<Any + Send + 'static>>,
    // !Send + !Sync
    _marker: PhantomData<*const ()>,
}

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

/// A handle to a scoped thread
pub struct ScopedJoinHandle<'scope, T: 'scope> {
    // !Send + !Sync
    inner: Rc<RefCell<Option<JoinState<T>>>>,
    thread: thread::Thread,
    _marker: PhantomData<&'scope T>,
}

unsafe impl<'scope, T> Send for ScopedJoinHandle<'scope, T> {}
unsafe impl<'scope, T> Sync for ScopedJoinHandle<'scope, T> {}

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
    let mut scope = Scope {
        joins: RefCell::new(Vec::new()),
        panics: Vec::new(),
        _marker: PhantomData,
    };

    // Executes the scoped function. Panic will be catched as `Err`.
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| f(&scope)));

    // Joins all the threads.
    scope.join_all();
    let panic = scope.panics.pop();

    // If any of the threads panicked, returns the panic's payload.
    if let Some(payload) = panic {
        return Err(payload);
    }

    // Returns the result of the scoped function.
    result
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
    // and, if any thread join panics, can be resumed in the unwinding this causes. By initially
    // running the method outside of any destructor, we avoid any leakage problems due to
    // @rust-lang/rust#14875.
    //
    // FIXME(jeehoonkang): @rust-lang/rust#14875 is fixed, so maybe we can remove the above comment.
    // But I'd like to write tests to check it before removing the comment.
    fn join_all(&mut self) {
        let mut joins = self.joins.borrow_mut();
        for join in joins.drain(..) {
            let result = join.call_box();
            if let Err(payload) = result {
                self.panics.push(payload);
            }
        }
    }

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

        self.scope.joins.borrow_mut().push(Box::new(move || {
            let state = deferred_handle.borrow_mut().deref_mut().take();
            if let Some(state) = state {
                state.join().map(|_| ())
            } else {
                Ok(())
            }
        }));

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
        // Note that `self.joins` can be non-empty when the code inside a `scope()` panics and
        // `drop()` is called in unwinding. Even if it's the case, we will join the unjoined
        // threads.
        //
        // We ignore panics from any threads because we're in course of unwinding anyway.
        self.join_all();
    }
}

type ScopedThreadResult<T> = Arc<Mutex<Option<T>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::{thread, time};

    const TIMES: usize = 10;
    const SMALL_STACK_SIZE: usize = 20;

    #[test]
    fn join() {
        let counter = AtomicUsize::new(0);
        scope(|scope| {
            let handle = scope.spawn(|| {
                counter.store(1, Ordering::Relaxed);
            });
            assert!(handle.join().is_ok());

            let panic_handle = scope.spawn(|| {
                panic!("\"My honey is running out!\", said Pooh.");
            });
            assert!(panic_handle.join().is_err());
        }).unwrap();

        // There should be sufficient synchronization.
        assert_eq!(1, counter.load(Ordering::Relaxed));
    }

    #[test]
    fn counter() {
        let counter = AtomicUsize::new(0);
        scope(|scope| {
            for _ in 0..TIMES {
                scope.spawn(|| {
                    counter.fetch_add(1, Ordering::Relaxed);
                });
            }
        }).unwrap();

        assert_eq!(TIMES, counter.load(Ordering::Relaxed));
    }

    #[test]
    fn counter_builder() {
        let counter = AtomicUsize::new(0);
        scope(|scope| {
            for i in 0..TIMES {
                scope
                    .builder()
                    .name(format!("child-{}", i))
                    .stack_size(SMALL_STACK_SIZE)
                    .spawn(|| {
                        counter.fetch_add(1, Ordering::Relaxed);
                    })
                    .unwrap();
            }
        }).unwrap();

        assert_eq!(TIMES, counter.load(Ordering::Relaxed));
    }

    #[test]
    fn counter_panic() {
        let counter = AtomicUsize::new(0);
        let result = scope(|scope| {
            scope.spawn(|| {
                panic!("\"My honey is running out!\", said Pooh.");
            });
            thread::sleep(time::Duration::from_millis(100));

            for _ in 0..TIMES {
                scope.spawn(|| {
                    counter.fetch_add(1, Ordering::Relaxed);
                });
            }
        });

        assert_eq!(TIMES, counter.load(Ordering::Relaxed));
        assert!(result.is_err());
    }

    #[test]
    fn panic_twice() {
        let result = scope(|scope| {
            scope.spawn(|| {
                panic!();
            });
            panic!();
        });
        assert!(result.is_err());
    }
}
