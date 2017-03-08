//! Scoped threads.

use std::boxed::FnBox;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::{mem, fmt, io, thread};

use builder_spawn_unsafe;
use sync::AtomicOption;

/// The scope of a thread.
///
/// A scope spans some lifetime and keeps track of destructors of the thread.
///
/// When this is dropped all the associated destructors are run in reverse (LIFO) order.
pub struct Scope<'a> {
    /// The list of destructors associated with this scope.
    dtors: RefCell<Option<DtorList<'a>>>,
}

/// A list of destructors.
struct DtorList<'a> {
    /// The head destructor.
    dtor: Box<FnBox + 'a>,
    /// The tail of the list.
    ///
    /// If `None`, this was the last link.
    next: Option<Box<DtorList<'a>>>,
}

/// The state of a thread.
///
/// This tracks if a thread has been joined to the parent or not.
enum JoinState {
    /// The thread is still running.
    ///
    /// The inner handle is the handle of the thread associated with this join state.
    Running(thread::JoinHandle<()>),
    /// The thread has been joined to the parent thread.
    Joined,
}

impl JoinState {
    /// Join the thread to the current thread.
    ///
    /// This blocks the current thread until the thread associated with `self` is completed.
    fn join(&mut self) {
        // Set the thread to joined.
        if let JoinState::Running(handle) = mem::replace(self, JoinState::Joined) {
            // The thread was not joined yet.

            // Wait through the handle.
            let res = handle.join();

            // Propagate panics.
            if !thread::panicking() {
                res.unwrap();
            }
        }
    }
}

/// A handle to a scoped thread.
///
/// This tracks the thread as well as its state, and allows doing various operations on the thread.
pub struct ScopedJoinHandle<T> {
    /// The join state of this thread.
    inner: Rc<RefCell<JoinState>>,
    /// The return value of the thread.
    ///
    /// This is `None` until the thread is completed.
    packet: Arc<AtomicOption<T>>,
    /// The thread itself.
    thread: thread::Thread,
}

/// Create a new `scope`, for deferred destructors.
///
/// Scopes, in particular, support [*scoped thread spawning*](struct.Scope.html#method.spawn).
///
/// # Examples
///
/// Creating and using a scope:
///
/// ```
/// crossbeam::scope(|scope| {
///     scope.defer(|| println!("Exiting scope"));
///     scope.spawn(|| println!("Running child thread in scope"))
/// });
/// // Prints messages in the reverse order written
/// ```
pub fn scope<'a, F, R>(f: F) -> R
    where F: FnOnce(&Scope<'a>) -> R {
    // Initialize the scope with no destructor.
    let mut scope = Scope { dtors: RefCell::new(None) };
    // Run the closure on the scope.
    let ret = f(&scope);
    // Run all the destructors.
    scope.drop_all();

    ret
}

impl<'a> fmt::Debug for Scope<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Scope {{ ... }}")
    }
}

impl<T> fmt::Debug for ScopedJoinHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ScopedJoinHandle {{ ... }}")
    }
}

impl<'a> Scope<'a> {
    /// Pop the last destructor added.
    fn pop_dtor(&self) -> Option<Box<FnBox>> {
        // Borrow the destructor list.
        let mut dtors = self.dtors.borrow_mut();

        if let Some(node) = dtors.take() {
            // Put the next node in the previous node's place.
            *dtors = node.next.take().map(|&b| b);

            Some(node.dtor)
        } else {
            // No more destructors.
            None
        }
    }

    /// Run all the destructors associated with this scope.
    ///
    /// This simply runs over all the destructors and calls them one-by-one sequentially in LIFO
    /// order.
    fn drop_all(&mut self) {
        // This method is carefully written in a transactional style, so that it can be called
        // directly and, if any dtor panics, can be resumed in the unwinding this causes. By
        // initially running the method outside of any destructor, we avoid any leakage problems
        // due to @rust-lang/rust#14875.

        loop {
            // Pop and run the top dtor.
            if let Some(x) = self.pop_dtors() { x } else {
                // No more destructors.
                break;
            }.call_box()
        }
    }

    /// Schedule code to be executed when exiting the scope.
    ///
    /// This is akin to having a destructor on the stack, except that it is *guaranteed* to be run.
    pub fn defer<F>(&self, f: F)
        where F: FnOnce() + 'a {
        // Get a reference to the dtor list.
        let mut dtors = self.dtors.borrow_mut();
        // Push the dtor to the list by updating the head node.
        *dtors = Some(DtorList {
            dtor: Box::new(f),
            next: dtors.take().map(Box::new),
        });
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
    /// `'static` lifetime. One way of giving it a proper lifetime is to use an [`Arc`][arc]:
    ///
    /// [arc]: http://doc.rust-lang.org/stable/std/sync/struct.Arc.html
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
    /// Enter scoped threads. Here's our original example, using `spawn` from crossbeam rather than
    /// from `std::thread`:
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
    pub fn spawn<F, T>(&self, f: F) -> ScopedJoinHandle<T>
        where F: FnOnce() -> T + Send + 'a,
              T: Send + 'a {
        self.builder().spawn(f).unwrap()
    }

    /// Create a thread builder.
    ///
    /// This generate the base configuration for spawning a scoped thread, from which configuration
    /// methods can be chained, following [the builder
    /// pattern](https://en.wikipedia.org/wiki/Builder_pattern).
    pub fn builder<'s>(&'s self) -> ScopedThreadBuilder<'s, 'a> {
        ScopedThreadBuilder {
            scope: self,
            builder: thread::Builder::new(),
        }
    }
}

/// A configuration of a scoped thread.
///
/// This provides detailed control over the properties and behavior of new scoped threads, allowing
/// the provided methods to update the configuration, and finally spawn the thread.
pub struct ScopedThreadBuilder<'s, 'a: 's> {
    /// The scope of the thread.
    scope: &'s Scope<'a>,
    /// The inner builder.
    builder: thread::Builder,
}

impl<'s, 'a: 's> ScopedThreadBuilder<'s, 'a> {
    /// Name the thread-to-be.
    ///
    /// Currently the name is used for identification only in panic messages, although future use
    /// could be more diverse.
    pub fn name(mut self, name: String) -> ScopedThreadBuilder<'s, 'a> {
        self.builder = self.builder.name(name);
        self
    }

    /// Set the size of the stack for the thread-to-be.
    ///
    /// Naturally `size` is in bytes.
    pub fn stack_size(mut self, size: usize) -> ScopedThreadBuilder<'s, 'a> {
        self.builder = self.builder.stack_size(size);
        self
    }

    /// Spawn a new thread with the configuration.
    ///
    /// This creates a new thread with the configuration given in `self` and returns its join
    /// handle (if spawned successfully).
    pub fn spawn<F, T>(self, f: F) -> io::Result<ScopedJoinHandle<T>>
        where F: FnOnce() -> T + Send + 'a,
              T: Send + 'a {
        // Create the packages, which will hold the return value of the thread..
        let their_packet = Arc::new(AtomicOption::new());
        let my_packet = their_packet.clone();

        // Spawn the thread.
        let join_handle = unsafe {
            // This is safe due to the bounds on this method, ensuring thread safety through the
            // type system.
            builder_spawn_unsafe(self.builder, move || {
                // As we don't need any particular ordering constraints, relaxed ordering
                // suffices.
                their_packet.swap(f(), Ordering::Relaxed);
            })
        }?;

        // Get the thread of the join handle
        let thread = join_handle.thread().clone();
        // Make the join handle compatible with a join state (set to not joined yet).
        let join_handle = Rc::new(RefCell::new(JoinState::Running(join_handle)));

        {
            // Acquire the handle to use in the destructor.
            let join_handle = join_handle.clone();
            // When destroyed, the scoped handle will call `join()` on the state (i.e. join if not
            // already done).
            self.scope.defer(move || {
                let mut state = join_handle.borrow_mut();
                state.join();
            });
        }

        Ok(ScopedJoinHandle {
            inner: my_handle,
            packet: my_packet,
            thread: thread,
        })
    }
}

impl<T> ScopedJoinHandle<T> {
    /// Block the current thread until the scoped thread finishes.
    ///
    /// This returns the result the inner thread produced.
    pub fn join(self) -> T {
        // Borrow and join.
        self.inner.borrow_mut().join();
        // Read the packet. The `unwrap()` should never panic, as the thread is wrapped such that
        // it puts its result in `Self::packet`.
        self.packet.take(Ordering::Relaxed).unwrap()
    }

    /// Get the underlying thread handle.
    pub fn thread(&self) -> &thread::Thread {
        &self.thread
    }
}

impl<'a> Drop for Scope<'a> {
    fn drop(&mut self) {
        // RUN ALL 'EM DTORS.
        self.drop_all()
    }
}
