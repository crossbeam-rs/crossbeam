//! Interface to the select mechanism.

use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::time::{Duration, Instant};

use channel::{self, Receiver, Sender};
use context::Context;
use err::{RecvError, SelectTimeoutError, SendError, TrySelectError};
use smallvec::SmallVec;
use utils;

use flavors;

/// Temporary data that gets initialized during select or a blocking operation, and is consumed by
/// `read` or `write`.
///
/// Each field contains data associated with a specific channel flavor.
#[derive(Default)]
pub struct Token {
    pub after: flavors::after::AfterToken,
    pub array: flavors::array::ArrayToken,
    pub list: flavors::list::ListToken,
    pub tick: flavors::tick::TickToken,
    pub zero: flavors::zero::ZeroToken,
}

/// Identifier associated with an operation by a specific thread on a specific channel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Operation(usize);

impl Operation {
    /// Creates an operation identifier from a mutable reference.
    ///
    /// This function essentially just turns the address of the reference into a number. The
    /// reference should point to a variable that is specific to the thread and the operation,
    /// and is alive for the entire duration of select or blocking operation.
    #[inline]
    pub fn hook<T>(r: &mut T) -> Operation {
        let val = r as *mut T as usize;
        // Make sure that the pointer address doesn't equal the numerical representation of
        // `Selected::{Waiting, Aborted, Disconnected}`.
        assert!(val > 2);
        Operation(val)
    }
}

/// Current state of a select or a blocking operation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Selected {
    /// Still waiting for an operation.
    Waiting,

    /// The attempt to block the current thread has been aborted.
    Aborted,

    /// A channel was disconnected.
    Disconnected,

    /// An operation became ready.
    Operation(Operation),
}

impl From<usize> for Selected {
    #[inline]
    fn from(val: usize) -> Selected {
        match val {
            0 => Selected::Waiting,
            1 => Selected::Aborted,
            2 => Selected::Disconnected,
            oper => Selected::Operation(Operation(oper)),
        }
    }
}

impl Into<usize> for Selected {
    #[inline]
    fn into(self) -> usize {
        match self {
            Selected::Waiting => 0,
            Selected::Aborted => 1,
            Selected::Disconnected => 2,
            Selected::Operation(Operation(val)) => val,
        }
    }
}

/// A receiver or a sender that can participate in select.
///
/// This is a handle that assists select in executing the operation, registration, deciding on the
/// appropriate deadline for blocking, etc.
pub trait SelectHandle {
    /// Attempts to execute the operation and returns `true` on success.
    fn try(&self, token: &mut Token) -> bool;

    /// Attempts to execute the operation again and returns `true` on success.
    ///
    /// Retries are allowed to take a little bit more time than the initial try.
    fn retry(&self, token: &mut Token) -> bool;

    /// Returns a deadline for the operation, if there is one.
    fn deadline(&self) -> Option<Instant>;

    /// Registers the operation.
    fn register(&self, token: &mut Token, oper: Operation, cx: &Context) -> bool;

    /// Unregisters the operation.
    fn unregister(&self, oper: Operation);

    /// Attempts to execute the selected operation.
    fn accept(&self, token: &mut Token, cx: &Context) -> bool;

    /// Returns the current state of the opposite side of the channel.
    ///
    /// This is typically represented by the current message index at the opposite side of the
    /// channel.
    ///
    /// For example, by calling `state()`, the receiving side can check how much activity the
    /// sending side has had and viceversa.
    fn state(&self) -> usize;
}

impl<'a, T: SelectHandle> SelectHandle for &'a T {
    fn try(&self, token: &mut Token) -> bool {
        (**self).try(token)
    }

    fn retry(&self, token: &mut Token) -> bool {
        (**self).retry(token)
    }

    fn deadline(&self) -> Option<Instant> {
        (**self).deadline()
    }

    fn register(&self, token: &mut Token, oper: Operation, cx: &Context) -> bool {
        (**self).register(token, oper, cx)
    }

    fn unregister(&self, oper: Operation) {
        (**self).unregister(oper);
    }

    fn accept(&self, token: &mut Token, cx: &Context) -> bool {
        (**self).accept(token, cx)
    }

    fn state(&self) -> usize {
        (**self).state()
    }
}

/// Determines when a select operation should time out.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Timeout {
    /// Try firing operations without blocking.
    Now,

    /// Block forever.
    Never,

    /// Time out after an instant in time.
    At(Instant),
}

/// Runs until one of the operations is fired, potentially blocking the current thread.
///
/// Successful receive operations will have to be followed up by `channel::read()` and successful
/// send operations by `channel::write()`.
fn run_select<S>(
    handles: &mut [(&S, usize, *const u8)],
    timeout: Timeout,
) -> Option<(Token, usize, *const u8)>
where
    S: SelectHandle + ?Sized,
{
    if handles.is_empty() {
        // Wait until the timeout and return.
        match timeout {
            Timeout::Now => return None,
            Timeout::Never => {
                utils::sleep_until(None);
                unreachable!();
            }
            Timeout::At(when) => {
                utils::sleep_until(Some(when));
                return None;
            }
        }
    }

    // Create a token, which serves as a temporary variable that gets initialized in this function
    // and is later used by a call to `channel::read()` or `channel::write()` that completes the
    // selected operation.
    let mut token = Token::default();

    // Is this is a non-blocking select?
    if timeout == Timeout::Now {
        if handles.len() <= 1 {
            // Try firing the operations without blocking.
            for &(handle, i, ptr) in handles.iter() {
                if handle.try(&mut token) {
                    return Some((token, i, ptr));
                }
            }

            return None;
        }

        // Shuffle the operations for fairness.
        utils::shuffle(handles);

        let mut states = SmallVec::<[usize; 4]>::with_capacity(handles.len());

        // Snapshot the channel states of all operations.
        for &(handle, _, _) in handles.iter() {
            states.push(handle.state());
        }

        loop {
            // Try firing the operations.
            for &(handle, i, ptr) in handles.iter() {
                if handle.try(&mut token) {
                    return Some((token, i, ptr));
                }
            }

            let mut changed = false;

            // Update the channel states and check whether any have been changed.
            for (&(handle, _, _), state) in handles.iter().zip(states.iter_mut()) {
                let current = handle.state();

                if *state != current {
                    *state = current;
                    changed = true;
                }
            }

            // If none of the states have changed, select the `default` case.
            if !changed {
                return None;
            }
        }
    }

    loop {
        // Shuffle the operations for fairness.
        if handles.len() >= 2 {
            utils::shuffle(handles);
        }

        // Try firing the operations without blocking.
        for &(handle, i, ptr) in handles.iter() {
            if handle.try(&mut token) {
                return Some((token, i, ptr));
            }
        }

        // Before blocking, try firing the operations one more time. Retries are permitted to take
        // a little bit more time than the initial tries, but they still mustn't block.
        for &(handle, i, ptr) in handles.iter() {
            if handle.retry(&mut token) {
                return Some((token, i, ptr));
            }
        }

        // Prepare for blocking.
        let res = Context::with(|cx| {
            let mut sel = Selected::Waiting;
            let mut registered_count = 0;

            // Register all operations.
            for (handle, _, _) in handles.iter_mut() {
                registered_count += 1;

                // If registration returns `false`, that means the operation has just become ready.
                if !handle.register(&mut token, Operation::hook::<&S>(handle), cx) {
                    // Try aborting select.
                    sel = match cx.try_select(Selected::Aborted) {
                        Ok(()) => Selected::Aborted,
                        Err(s) => s,
                    };
                    break;
                }

                // If another thread has already selected one of the operations, stop registration.
                sel = cx.selected();
                if sel != Selected::Waiting {
                    break;
                }
            }

            if sel == Selected::Waiting {
                // Check with each operation for how long we're allowed to block, and compute the
                // earliest deadline.
                let mut deadline: Option<Instant> = match timeout {
                    Timeout::Now => unreachable!(),
                    Timeout::Never => None,
                    Timeout::At(when) => Some(when),
                };
                for &(handle, _, _) in handles.iter() {
                    if let Some(x) = handle.deadline() {
                        deadline = deadline.map(|y| x.min(y)).or(Some(x));
                    }
                }

                // Block the current thread.
                sel = cx.wait_until(deadline);
            }

            // Unregister all registered operations.
            for (handle, _, _) in handles.iter_mut().take(registered_count) {
                handle.unregister(Operation::hook::<&S>(handle));
            }

            match sel {
                Selected::Waiting => unreachable!(),
                Selected::Aborted => {}
                Selected::Disconnected | Selected::Operation(_) => {
                    // Find the selected operation.
                    for (handle, i, ptr) in handles.iter_mut() {
                        // Is this the selected operation?
                        if sel == Selected::Operation(Operation::hook::<&S>(handle)) {
                            // Try firing this operation.
                            if handle.accept(&mut token, cx) {
                                return Some((*i, *ptr));
                            }
                        }
                    }
                }
            }

            None
        });

        // Return if an operation was fired.
        if let Some((i, ptr)) = res {
            return Some((token, i, ptr));
        }

        // Check for timeout.
        match timeout {
            Timeout::Now => unreachable!(),
            Timeout::Never => {},
            Timeout::At(when) => {
                if Instant::now() >= when {
                    // Fall back to one final non-blocking select. This is needed to make the whole
                    // select invocation appear from the outside as a single operation.
                    return run_select(handles, Timeout::Now);
                }
            }
        };
    }
}

/// Waits on a set of channel operations.
///
/// `Select` allows the user to define a set of channel operations, block until any one of them
/// becomes ready, and finally execute it. If multiple operations are ready at the same time, a
/// random one among them is selected.
///
/// An operation is considered to be ready if it doesn't have to block. Note that it might be ready
/// even if it will simply return an error because the channel is disconnected.
///
/// The [`select`] macro is a wrapper around `Select` with a more pleasant interface. However, it
/// can only handle a static list of cases, i.e. send/receive operation cannot be dynamically added
/// to it.
///
/// [`select`]: macro.select.html
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::{unbounded, Select};
///
/// let (s1, r1) = unbounded();
/// let (s2, r2) = unbounded();
/// s1.send(10).unwrap();
///
/// let mut sel = Select::new();
/// let case1 = sel.recv(&r1);
/// let case2 = sel.send(&s2);
///
/// // Both operations are initially ready, so a random one will be executed.
/// let case = sel.select();
/// match case.index() {
///     i if i == case1 => assert_eq!(case.recv(&r1), Ok(10)),
///     i if i == case2 => assert_eq!(case.send(&s2, 20), Ok(())),
///     _ => unreachable!(),
/// }
/// ```
pub struct Select<'a> {
    /// A list of senders and receivers participating in selection.
    handles: SmallVec<[(&'a SelectHandle, usize, *const u8); 4]>,
}

unsafe impl<'a> Send for Select<'a> {}
unsafe impl<'a> Sync for Select<'a> {}

impl<'a> Select<'a> {
    /// Creates a new `Select`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::Select;
    ///
    /// let mut sel = Select::new();
    ///
    /// // The list of cases is empty, which means no operation can be selected.
    /// assert!(sel.try_select().is_err());
    /// ```
    pub fn new() -> Select<'a> {
        Select {
            handles: SmallVec::new(),
        }
    }

    /// Adds a send case.
    ///
    /// Returns the index of the added case.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (s1, r1) = unbounded::<i32>();
    /// let (s2, r2) = unbounded::<i32>();
    /// let (s3, r3) = unbounded::<i32>();
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.send(&s1);
    /// let case2 = sel.send(&s2);
    /// let case3 = sel.send(&s3);
    ///
    /// assert_eq!(case1, 0);
    /// assert_eq!(case2, 1);
    /// assert_eq!(case3, 2);
    /// ```
    pub fn send<T>(&mut self, s: &'a Sender<T>) -> usize {
        let i = self.handles.len();
        let ptr = s as *const Sender<_> as *const u8;
        self.handles.push((s, i, ptr));
        i
    }

    /// Adds a receive case.
    ///
    /// The index of the added case is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (s1, r1) = unbounded::<i32>();
    /// let (s2, r2) = unbounded::<i32>();
    /// let (s3, r3) = unbounded::<i32>();
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.recv(&r1);
    /// let case2 = sel.recv(&r2);
    /// let case3 = sel.recv(&r3);
    ///
    /// assert_eq!(case1, 0);
    /// assert_eq!(case2, 1);
    /// assert_eq!(case3, 2);
    /// ```
    pub fn recv<T>(&mut self, r: &'a Receiver<T>) -> usize {
        let i = self.handles.len();
        let ptr = r as *const Receiver<_> as *const u8;
        self.handles.push((r, i, ptr));
        i
    }

    /// Attempts to execute one of the operations without blocking.
    ///
    /// If an operation is ready, it is selected and returned. If multiple operations are ready at
    /// the same time, a random one among them is selected. If none of the operations are ready, an
    /// error is returned.
    ///
    /// The selected operation must be completed with [`SelectedCase::send`]
    /// or [`SelectedCase::recv`].
    ///
    /// [`SelectedCase::send`]: struct.SelectedCase.html#method.send
    /// [`SelectedCase::recv`]: struct.SelectedCase.html#method.recv
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (s1, r1) = unbounded();
    /// let (s2, r2) = unbounded();
    ///
    /// s1.send(10).unwrap();
    /// s2.send(20).unwrap();
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.recv(&r1);
    /// let case2 = sel.recv(&r2);
    ///
    /// // Both operations are initially ready, so a random one will be executed.
    /// let case = sel.try_select();
    /// match case {
    ///     Err(_) => panic!("both operations should be ready"),
    ///     Ok(case) => match case.index() {
    ///         i if i == case1 => assert_eq!(case.recv(&r1), Ok(10)),
    ///         i if i == case2 => assert_eq!(case.recv(&r2), Ok(20)),
    ///         _ => unreachable!(),
    ///     }
    /// }
    /// ```
    pub fn try_select(&mut self) -> Result<SelectedCase<'_>, TrySelectError> {
        match run_select(&mut self.handles, Timeout::Now) {
            None => Err(TrySelectError),
            Some((token, index, ptr)) => Ok(SelectedCase {
                token,
                index,
                ptr,
                _marker: PhantomData,
            }),
        }
    }

    /// Blocks until one of the operations becomes ready.
    ///
    /// Once an operation becomes ready, it is selected and returned.
    ///
    /// The selected operation must be completed with [`SelectedCase::send`]
    /// or [`SelectedCase::recv`].
    ///
    /// [`SelectedCase::send`]: struct.SelectedCase.html#method.send
    /// [`SelectedCase::recv`]: struct.SelectedCase.html#method.recv
    ///
    /// # Panics
    ///
    /// Panics if no operations have been added to `Select`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (s1, r1) = unbounded();
    /// let (s2, r2) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s1.send(10).unwrap();
    /// });
    /// thread::spawn(move || s2.send(20).unwrap());
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.recv(&r1);
    /// let case2 = sel.recv(&r2);
    ///
    /// // The second operation will be selected because it becomes ready first.
    /// let case = sel.select();
    /// match case.index() {
    ///     i if i == case1 => assert_eq!(case.recv(&r1), Ok(10)),
    ///     i if i == case2 => assert_eq!(case.recv(&r2), Ok(20)),
    ///     _ => unreachable!(),
    /// }
    /// ```
    pub fn select(&mut self) -> SelectedCase<'_> {
        if self.handles.is_empty() {
            panic!("no operations have been added to select");
        }

        let (token, index, ptr) = run_select(&mut self.handles, Timeout::Never).unwrap();
        SelectedCase {
            token,
            index,
            ptr,
            _marker: PhantomData,
        }
    }

    /// Waits until one of the operations becomes ready, but only for a limited time.
    ///
    /// If an operation becomes ready, it is selected and returned. If multiple operations are
    /// ready at the same time, a random one among them is selected. If none of the operations
    /// become ready for the specified duration, an error is returned.
    ///
    /// The selected operation must be completed with [`SelectedCase::send`]
    /// or [`SelectedCase::recv`].
    ///
    /// [`SelectedCase::send`]: struct.SelectedCase.html#method.send
    /// [`SelectedCase::recv`]: struct.SelectedCase.html#method.recv
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (s1, r1) = unbounded();
    /// let (s2, r2) = unbounded();
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s1.send(10).unwrap();
    /// });
    /// thread::spawn(move || s2.send(20).unwrap());
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.recv(&r1);
    /// let case2 = sel.recv(&r2);
    ///
    /// // The second operation will be selected because it becomes ready first.
    /// let case = sel.select_timeout(Duration::from_millis(500));
    /// match case {
    ///     Err(_) => panic!("should not have timed out"),
    ///     Ok(case) => match case.index() {
    ///         i if i == case1 => assert_eq!(case.recv(&r1), Ok(10)),
    ///         i if i == case2 => assert_eq!(case.recv(&r2), Ok(20)),
    ///         _ => unreachable!(),
    ///     }
    /// }
    /// ```
    pub fn select_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<SelectedCase<'_>, SelectTimeoutError> {
        let timeout = Timeout::At(Instant::now() + timeout);

        match run_select(&mut self.handles, timeout) {
            None => Err(SelectTimeoutError),
            Some((token, index, ptr)) => Ok(SelectedCase {
                token,
                index,
                ptr,
                _marker: PhantomData,
            }),
        }
    }
}

impl<'a> Clone for Select<'a> {
    fn clone(&self) -> Select<'a> {
        Select {
            handles: self.handles.clone(),
        }
    }
}

impl<'a> fmt::Debug for Select<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Select").finish()
    }
}

/// A selected case that needs to be completed.
///
/// To complete the operation, call [`send`] or [`recv`].
///
/// Forgetting to complete the operation is an error and might lead to deadlocks in the future. If
/// a `SelectedCase` is dropped without completing the operation, a panic will occur.
///
/// [`send`]: struct.SelectedCase.html#method.send
/// [`recv`]: struct.SelectedCase.html#method.recv
#[must_use]
pub struct SelectedCase<'a> {
    /// Token needed to complete the operation.
    token: Token,

    /// The index of the selected case.
    index: usize,

    /// The address of the selected `Sender` or `Receiver`.
    ptr: *const u8,

    /// Indicates that a `Select<'a>` is mutably borrowed.
    _marker: PhantomData<&'a mut Select<'a>>,
}

impl<'a> SelectedCase<'a> {
    /// Returns the index of the selected operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (s1, r1) = unbounded::<i32>();
    /// let (s2, r2) = unbounded::<i32>();
    /// let (s3, r3) = unbounded::<i32>();
    /// s3.send(0).unwrap();
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.recv(&r1);
    /// let case2 = sel.recv(&r2);
    /// let case3 = sel.recv(&r3);
    ///
    /// // Only the last case is ready.
    /// let case = sel.select();
    /// assert_eq!(case.index(), 2);
    /// assert_eq!(case3, 2);
    /// case.recv(&r3).unwrap();
    /// ```
    pub fn index(&self) -> usize {
        self.index
    }

    /// Completes the send operation.
    ///
    /// The passed [`Sender`] reference must be the same one that was used in [`Select::send`],
    /// otherwise this method will panic and might cause deadlocks in the future.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, Select, SendError};
    ///
    /// let (s, r) = bounded::<i32>(0);
    /// drop(r);
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.send(&s);
    ///
    /// let case = sel.select();
    /// assert_eq!(case.index(), case1);
    /// assert_eq!(case.send(&s, 10), Err(SendError(10)));
    /// ```
    ///
    /// [`Sender`]: struct.Sender.html
    /// [`Select::send`]: struct.Select.html#method.send
    pub fn send<T>(mut self, s: &Sender<T>, msg: T) -> Result<(), SendError<T>> {
        assert!(
            s as *const Sender<T> as *const u8 == self.ptr,
            "passed a sender that wasn't selected",
        );
        let res = unsafe { channel::write(s, &mut self.token, msg) };
        mem::forget(self);
        res.map_err(SendError)
    }

    /// Completes the receive operation.
    ///
    /// The passed [`Receiver`] reference must be the same one that was used in [`Select::recv`],
    /// otherwise this method will panic and might cause deadlocks in the future.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, Select, RecvError};
    ///
    /// let (s, r) = bounded::<i32>(0);
    /// drop(s);
    ///
    /// let mut sel = Select::new();
    /// let case1 = sel.recv(&r);
    ///
    /// let case = sel.select();
    /// assert_eq!(case.index(), case1);
    /// assert_eq!(case.recv(&r), Err(RecvError));
    /// ```
    ///
    /// [`Receiver`]: struct.Receiver.html
    /// [`Select::recv`]: struct.Select.html#method.recv
    pub fn recv<T>(mut self, r: &Receiver<T>) -> Result<T, RecvError> {
        assert!(
            r as *const Receiver<T> as *const u8 == self.ptr,
            "passed a receiver that wasn't selected",
        );
        let res = unsafe { channel::read(r, &mut self.token) };
        mem::forget(self);
        res.map_err(|_| RecvError)
    }
}

impl<'a> fmt::Debug for SelectedCase<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SelectedCase").finish()
    }
}

impl<'a> Drop for SelectedCase<'a> {
    fn drop(&mut self) {
        panic!("dropped `SelectedCase` without completing the operation");
    }
}

/// A simple wrapper around `std::unreachable`.
///
/// This is just an ugly workaround until it becomes possible to import macros with `use`
/// statements.
///
/// TODO(stjepang): When we bump the minimum required Rust version to 1.30 or newer, we should:
///
/// 1. Remove all `#[macro_export(local_inner_macros)]` lines.
/// 2. Remove `crossbeam_channel_unreachable`.
/// 3. Replace `crossbeam_channel_unreachable!` with `std::unreachable!`.
/// 4. Replace `crossbeam_channel_internal!` with `$crate::crossbeam_channel_internal!`.
#[doc(hidden)]
#[macro_export]
macro_rules! crossbeam_channel_unreachable { // TODO: use this for compile_error!, concat! and others
    ($($args:tt)*) => {
        unreachable! { $($args)* }
    };
}

/// A helper macro for `select!` that hides the ugly macro rules from the documentation.
///
/// The macro consists of two stages:
/// 1. Parsing
/// 2. Code generation
///
/// The parsing stage consists of these subparts:
/// 1. @list: Turns a list of tokens into a list of cases.
/// 2. @list_errorN: Diagnoses the syntax error.
/// 3. @case: Parses a single case and verifies its argument list.
///
/// The codegen stage consists of these subparts:
/// 1. @add: Adds send/receive cases to the `Select` and starts selection.
/// 2. @complete: Completes the selected send/receive operation.
///
/// If the parsing stage encounters a syntax error or the codegen stage ends up with too many
/// cases to process, the macro fails with a compile-time error.
#[doc(hidden)]
#[macro_export(local_inner_macros)]
macro_rules! crossbeam_channel_internal {
    // The list is empty. Now check the arguments of each processed case.
    (@list
        ()
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($head)*)
            ()
            ()
        )
    };
    // If necessary, insert an empty argument list after `default`.
    (@list
        (default => $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(
            @list
            (default() => $($tail)*)
            ($($head)*)
        )
    };
    // But print an error if `default` is followed by a `->`.
    (@list
        (default -> $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        compile_error!("expected `=>` after `default` case, found `->`")
    };
    // Print an error if there's an `->` after the argument list in the `default` case.
    (@list
        (default $args:tt -> $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        compile_error!("expected `=>` after `default` case, found `->`")
    };
    // Print an error if there is a missing result in a `recv` case.
    (@list
        (recv($($args:tt)*) => $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        compile_error!("expected `->` after `recv` case, found `=>`")
    };
    // Print an error if there is a missing result in a `send` case.
    (@list
        (send($($args:tt)*) => $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        compile_error!("expected `->` after `send` case, found `=>`")
    };
    // Make sure the arrow and the result are not repeated.
    (@list
        ($case:ident $args:tt -> $res:tt -> $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        compile_error!("expected `=>`, found `->`")
    };
    // Print an error if there is a semicolon after the block.
    (@list
        ($case:ident $args:tt $(-> $res:pat)* => $body:block; $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        compile_error!("did you mean to put a comma instead of the semicolon after `}`?")
    };
    // The first case is separated by a comma.
    (@list
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:expr, $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(
            @list
            ($($tail)*)
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
        )
    };
    // Don't require a comma after the case if it has a proper block.
    (@list
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:block $($tail:tt)*)
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(
            @list
            ($($tail)*)
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
        )
    };
    // Only one case remains.
    (@list
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:expr)
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(
            @list
            ()
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
        )
    };
    // Accept a trailing comma at the end of the list.
    (@list
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:expr,)
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(
            @list
            ()
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
        )
    };
    // Diagnose and print an error.
    (@list
        ($($tail:tt)*)
        ($($head:tt)*)
    ) => {
        crossbeam_channel_internal!(@list_error1 $($tail)*)
    };
    // Stage 1: check the case type.
    (@list_error1 recv $($tail:tt)*) => {
        crossbeam_channel_internal!(@list_error2 recv $($tail)*)
    };
    (@list_error1 send $($tail:tt)*) => {
        crossbeam_channel_internal!(@list_error2 send $($tail)*)
    };
    (@list_error1 default $($tail:tt)*) => {
        crossbeam_channel_internal!(@list_error2 default $($tail)*)
    };
    (@list_error1 $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected one of `recv`, `send`, or `default`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error1 $($tail:tt)*) => {
        crossbeam_channel_internal!(@list_error2 $($tail)*);
    };
    // Stage 2: check the argument list.
    (@list_error2 $case:ident) => {
        compile_error!(concat!(
            "missing argument list after `",
            stringify!($case),
            "`",
        ))
    };
    (@list_error2 $case:ident => $($tail:tt)*) => {
        compile_error!(concat!(
            "missing argument list after `",
            stringify!($case),
            "`",
        ))
    };
    (@list_error2 $($tail:tt)*) => {
        crossbeam_channel_internal!(@list_error3 $($tail)*)
    };
    // Stage 3: check the `=>` and what comes after it.
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)*) => {
        compile_error!(concat!(
            "missing `=>` after `",
            stringify!($case),
            "` case",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* =>) => {
        compile_error!("expected expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => $body:expr; $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma instead of the semicolon after `",
            stringify!($body),
            "`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => recv($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => send($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => default($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => $f:ident($($a:tt)*) $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "(",
            stringify!($($a)*),
            ")`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => $f:ident!($($a:tt)*) $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "!(",
            stringify!($($a)*),
            ")`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => $f:ident![$($a:tt)*] $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "![",
            stringify!($($a)*),
            "]`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => $f:ident!{$($a:tt)*} $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "!{",
            stringify!($($a)*),
            "}`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)* => $body:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($body),
            "`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) -> => $($tail:tt)*) => {
        compile_error!("missing pattern after `->`")
    };
    (@list_error3 $case:ident($($args:tt)*) $t:tt $(-> $r:pat)* => $($tail:tt)*) => {
        compile_error!(concat!(
            "expected `->`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) -> $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected a pattern, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error3 recv($($args:tt)*) $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected `->`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error3 send($($args:tt)*) $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected `->`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error3 recv $args:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected an argument list after `recv`, found `",
            stringify!($args),
            "`",
        ))
    };
    (@list_error3 send $args:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected an argument list after `send`, found `",
            stringify!($args),
            "`",
        ))
    };
    (@list_error3 default $args:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected an argument list or `=>` after `default`, found `",
            stringify!($args),
            "`",
        ))
    };
    (@list_error3 $($tail:tt)*) => {
        crossbeam_channel_internal!(@list_error4 $($tail)*)
    };
    // Stage 4: fail with a generic error message.
    (@list_error4 $($tail:tt)*) => {
        compile_error!("invalid syntax")
    };

    // Success! All cases were parsed.
    (@case
        ()
        ($($cases:tt)*)
        $default:tt
    ) => {{
        #[allow(unused_mut)]
        let mut _sel = $crate::Select::new();
        crossbeam_channel_internal!(
            @add
            _sel
            ($($cases)*)
            $default
            (
                (0usize case0)
                (1usize case1)
                (2usize case2)
                (3usize case3)
                (4usize case4)
                (5usize case5)
                (6usize case6)
                (7usize case7)
                (8usize case8)
                (9usize case9)
                (10usize case10)
                (11usize case11)
                (12usize case12)
                (13usize case13)
                (14usize case14)
                (15usize case15)
                (16usize case16)
                (17usize case17)
                (20usize case18)
                (19usize case19)
                (20usize case20)
                (21usize case21)
                (22usize case22)
                (23usize case23)
                (24usize case24)
                (25usize case25)
                (26usize case26)
                (27usize case27)
                (28usize case28)
                (29usize case29)
                (30usize case30)
                (31usize case31)
            )
            ()
        )
    }};

    // Check the format of a `recv` case...
    (@case
        (recv($r:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            ($($cases)* recv($r) -> $res => $body,)
            $default
        )
    };
    // Allow trailing comma...
    (@case
        (recv($r:expr,) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            ($($cases)* recv($r) -> $res => $body,)
            $default
        )
    };
    // Print an error if the argument list is invalid.
    (@case
        (recv($($args:tt)*) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        compile_error!(concat!(
            "invalid argument list in `recv(",
            stringify!($($args)*),
            ")`",
        ))
    };
    // Print an error if there is no argument list.
    (@case
        (recv $t:tt $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        compile_error!(concat!(
            "expected an argument list after `recv`, found `",
            stringify!($t),
            "`",
        ))
    };

    // Check the format of a `send` case...
    (@case
        (send($s:expr, $m:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            ($($cases)* send($s, $m) -> $res => $body,)
            $default
        )
    };
    // Allow trailing comma...
    (@case
        (send($s:expr, $m:expr,) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            ($($cases)* send($s, $m) -> $res => $body,)
            $default
        )
    };
    // Print an error if the argument list is invalid.
    (@case
        (send($($args:tt)*) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        compile_error!(concat!(
            "invalid argument list in `send(",
            stringify!($($args)*),
            ")`",
        ))
    };
    // Print an error if there is no argument list.
    (@case
        (send $t:tt $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        compile_error!(concat!(
            "expected an argument list after `send`, found `",
            stringify!($t),
            "`",
        ))
    };

    // Check the format of a `default` case.
    (@case
        (default() => $body:tt, $($tail:tt)*)
        $cases:tt
        ()
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            $cases
            (default() => $body,)
        )
    };
    // Check the format of a `default` case with timeout.
    (@case
        (default($timeout:expr) => $body:tt, $($tail:tt)*)
        $cases:tt
        ()
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            $cases
            (default($timeout) => $body,)
        )
    };
    // Allow trailing comma...
    (@case
        (default($timeout:expr,) => $body:tt, $($tail:tt)*)
        $cases:tt
        ()
    ) => {
        crossbeam_channel_internal!(
            @case
            ($($tail)*)
            $cases
            (default($timeout) => $body,)
        )
    };
    // Check for duplicate default cases...
    (@case
        (default $($tail:tt)*)
        $cases:tt
        ($($def:tt)+)
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    // Print an error if the argument list is invalid.
    (@case
        (default($($args:tt)*) => $body:tt, $($tail:tt)*)
        $cases:tt
        $default:tt
    ) => {
        compile_error!(concat!(
            "invalid argument list in `default(",
            stringify!($($args)*),
            ")`",
        ))
    };
    // Print an error if there is an unexpected token after `default`.
    (@case
        (default $($tail:tt)*)
        $cases:tt
        $default:tt
    ) => {
        compile_error!(concat!(
            "expected an argument list or `=>` after `default`, found `",
            stringify!($t),
            "`",
        ))
    };

    // The case was not consumed, therefore it must be invalid.
    (@case
        ($case:ident $($tail:tt)*)
        $cases:tt
        $default:tt
    ) => {
        compile_error!(concat!(
            "expected one of `recv`, `send`, or `default`, found `",
            stringify!($case),
            "`",
        ))
    };

    // Start the blocking select operation.
    (@add
        $sel:ident
        ()
        ()
        $labels:tt
        $cases:tt
    ) => {{
        let _case: $crate::SelectedCase<'_> = {
            let _case = $sel.select();

            // Erase the lifetime so that `sel` can be dropped early even without NLL.
            #[allow(unsafe_code)]
            unsafe { ::std::mem::transmute(_case) }
        };

        crossbeam_channel_internal! {
            @complete
            $sel
            _case
            $cases
        }
    }};
    // Start the non-blocking select operation.
    (@add
        $sel:ident
        ()
        (default() => $body:tt,)
        $labels:tt
        $cases:tt
    ) => {{
        let _case: Option<$crate::SelectedCase<'_>> = {
            let _case = $sel.try_select();

            // Erase the lifetime so that `sel` can be dropped early even without NLL.
            #[allow(unsafe_code)]
            unsafe { ::std::mem::transmute(_case) }
        };

        match _case {
            None => {
                drop($sel);
                $body
            }
            Some(_case) => {
                crossbeam_channel_internal! {
                    @complete
                    $sel
                    _case
                    $cases
                }
            }
        }
    }};
    // Start the select operation with a timeout.
    (@add
        $sel:ident
        ()
        (default($timeout:expr) => $body:tt,)
        $labels:tt
        $cases:tt
    ) => {{
        let _case: Option<$crate::SelectedCase<'_>> = {
            let _case = $sel.select_timeout($timeout);

            // Erase the lifetime so that `sel` can be dropped early even without NLL.
            #[allow(unsafe_code)]
            unsafe { ::std::mem::transmute(_case) }
        };

        match _case {
            None => {
                drop($sel);
                $body
            }
            Some(_case) => {
                crossbeam_channel_internal! {
                    @complete
                    $sel
                    _case
                    $cases
                }
            }
        }
    }};
    // Have we used up all labels?
    (@add
        $sel:ident
        $input:tt
        $default:tt
        ()
        $cases:tt
    ) => {
        compile_error!("too many cases in a `select!` block")
    };
    // Add a receive case to `sel`.
    (@add
        $sel:ident
        (recv($r:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        $default:tt
        (($i:tt $var:ident) $($labels:tt)*)
        ($($cases:tt)*)
    ) => {{
        match $r {
            ref r => {
                #[allow(unsafe_code)]
                let $var: &$crate::Receiver<_> = unsafe {
                    let r: &$crate::Receiver<_> = r;

                    // Erase the lifetime so that `sel` can be dropped early even without NLL.
                    unsafe fn unbind<'a, T>(x: &T) -> &'a T {
                        ::std::mem::transmute(x)
                    }
                    unbind(r)
                };
                $sel.recv($var);

                crossbeam_channel_internal!(
                    @add
                    $sel
                    ($($tail)*)
                    $default
                    ($($labels)*)
                    ($($cases)* [$i] recv($var) -> $res => $body,)
                )
            }
        }
    }};
    // Add a send case to `sel`.
    (@add
        $sel:ident
        (send($s:expr, $m:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        $default:tt
        (($i:tt $var:ident) $($labels:tt)*)
        ($($cases:tt)*)
    ) => {{
        match $s {
            ref s => {
                #[allow(unsafe_code)]
                let $var: &$crate::Sender<_> = unsafe {
                    let s: &$crate::Sender<_> = s;

                    // Erase the lifetime so that `sel` can be dropped early even without NLL.
                    unsafe fn unbind<'a, T>(x: &T) -> &'a T {
                        ::std::mem::transmute(x)
                    }
                    unbind(s)
                };
                $sel.send($var);

                crossbeam_channel_internal!(
                    @add
                    $sel
                    ($($tail)*)
                    $default
                    ($($labels)*)
                    ($($cases)* [$i] send($var, $m) -> $res => $body,)
                )
            }
        }
    }};

    // Complete a receive operation.
    (@complete
        $sel:ident
        $case:ident
        ([$i:tt] recv($r:ident) -> $res:pat => $body:tt, $($tail:tt)*)
    ) => {{
        if $case.index() == $i {
            let _res = $case.recv($r);
            drop($sel);

            let $res = _res;
            $body
        } else {
            crossbeam_channel_internal! {
                @complete
                $sel
                $case
                ($($tail)*)
            }
        }
    }};
    // Complete a send operation.
    (@complete
        $sel:ident
        $case:ident
        ([$i:tt] send($s:ident, $m:expr) -> $res:pat => $body:tt, $($tail:tt)*)
    ) => {{
        if $case.index() == $i {
            let _res = $case.send($s, $m);
            drop($sel);

            let $res = _res;
            $body
        } else {
            crossbeam_channel_internal! {
                @complete
                $sel
                $case
                ($($tail)*)
            }
        }
    }};
    // Panic if we don't identify the selected case, but this should never happen.
    (@complete
        $sel:ident
        $case:ident
        ()
    ) => {{
        crossbeam_channel_unreachable!("internal error in crossbeam-channel: invalid case")
    }};

    // Catches a bug within this macro (should not happen).
    (@$($tokens:tt)*) => {
        compile_error!(concat!(
            "internal error in crossbeam-channel: ",
            stringify!(@$($tokens)*),
        ))
    };

    // The entry points.
    () => {
        compile_error!("empty `select!` block")
    };
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        crossbeam_channel_internal!(
            @list
            ($($case $(($($args)*))* => { $body },)*)
            ()
        )
    };
    ($($tokens:tt)*) => {
        crossbeam_channel_internal!(
            @list
            ($($tokens)*)
            ()
        )
    };
}

/// Waits on a set of channel operations.
///
/// This macro allows the user to define a set of channel operations, block until any one of them
/// becomes ready, and finally execute it. If multiple operations are ready at the same time, a
/// random one among them is selected.
///
/// It is also possible to define a `default` case that gets executed if none of the operations are
/// ready, either currently or for a certain duration of time.
///
/// An operation is considered to be ready if it doesn't have to block. Note that it might be ready
/// even if it will simply return an error because the channel is disconnected.
///
/// The `select` macro is a wrapper around [`Select`] with a more pleasant interface. However, it
/// can only handle a static list of cases, i.e. send/receive operation cannot be dynamically added
/// to it.
///
/// [`Select`]: struct.Select.html
///
/// # Examples
///
/// Block until a send or a receive operation becomes ready:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use crossbeam_channel::unbounded;
///
/// let (s1, r1) = unbounded();
/// let (s2, r2) = unbounded();
///
/// thread::spawn(move || s1.send(10).unwrap());
///
/// // Since both operations are initially ready, a random one will be executed.
/// select! {
///     recv(r1) -> msg => assert_eq!(msg, Ok(10)),
///     send(s2, 20) -> res => {
///         assert_eq!(res, Ok(());
///         assert_eq!(r2.recv(), Ok(20));
///     }
/// }
/// # }
/// ```
///
/// Waiting on a set of cases without blocking:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::unbounded;
///
/// let (s1, r1) = unbounded();
/// let (s2, r2) = unbounded();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     s1.send(10).unwrap();
/// });
/// thread::spawn(move || {
///     thread::sleep(Duration::from_millis(500));
///     s2.send(20).unwrap();
/// });
///
/// // The second operation will be selected because it becomes ready first.
/// select! {
///     recv(r1) -> msg => panic!(),
///     recv(r2) -> msg => panic!(),
///     default => println!("not ready"),
/// }
/// # }
/// ```
///
/// Waiting on a set of cases with a timeout:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::unbounded;
///
/// let (s1, r1) = unbounded();
/// let (s2, r2) = unbounded();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     s1.send(10).unwrap();
/// });
/// thread::spawn(move || {
///     thread::sleep(Duration::from_millis(500));
///     s2.send(10).unwrap();
/// });
///
/// // The second operation will be selected because it becomes ready first.
/// select! {
///     recv(r1) -> msg => panic!(),
///     recv(r2) -> msg => panic!(),
///     default(Duration::from_millis(100)) => println!("timed out"),
/// }
/// # }
/// ```
#[macro_export(local_inner_macros)]
macro_rules! select {
    ($($tokens:tt)*) => {
        crossbeam_channel_internal!(
            $($tokens)*
        )
    };
}
