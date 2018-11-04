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
    pub never: flavors::never::NeverToken,
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

            // If none of the states have changed, selection failed.
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

/// Selects from a set of channel operations.
///
/// `Select` allows you to define a set of channel operations, wait until any one of them becomes
/// ready, and finally execute it. If multiple operations are ready at the same time, a random one
/// among them is selected.
///
/// An operation is considered to be ready if it doesn't have to block. Note that it is ready even
/// when it will simply return an error because the channel is disconnected.
///
/// The [`select!`] macro is a convenience wrapper around `Select`. However, it cannot select over a
/// dynamically created list of channel operations.
///
/// [`select!`]: macro.select.html
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
/// let oper1 = sel.recv(&r1);
/// let oper2 = sel.send(&s2);
///
/// // Both operations are initially ready, so a random one will be executed.
/// let oper = sel.select();
/// match oper.index() {
///     i if i == oper1 => assert_eq!(oper.recv(&r1), Ok(10)),
///     i if i == oper2 => assert_eq!(oper.send(&s2, 20), Ok(())),
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
    /// Creates an empty list of channel operations for selection.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::Select;
    ///
    /// let mut sel = Select::new();
    ///
    /// // The list of operations is empty, which means no operation can be selected.
    /// assert!(sel.try_select().is_err());
    /// ```
    pub fn new() -> Select<'a> {
        Select {
            handles: SmallVec::new(),
        }
    }

    /// Adds a send operation.
    ///
    /// Returns the index of the added operation.
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
    /// let oper1 = sel.send(&s1);
    /// let oper2 = sel.send(&s2);
    /// let oper3 = sel.send(&s3);
    ///
    /// assert_eq!(oper1, 0);
    /// assert_eq!(oper2, 1);
    /// assert_eq!(oper3, 2);
    /// ```
    pub fn send<T>(&mut self, s: &'a Sender<T>) -> usize {
        let i = self.handles.len();
        let ptr = s as *const Sender<_> as *const u8;
        self.handles.push((s, i, ptr));
        i
    }

    /// Adds a receive operation.
    ///
    /// Returns the index of the added operation.
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
    /// let oper1 = sel.recv(&r1);
    /// let oper2 = sel.recv(&r2);
    /// let oper3 = sel.recv(&r3);
    ///
    /// assert_eq!(oper1, 0);
    /// assert_eq!(oper2, 1);
    /// assert_eq!(oper3, 2);
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
    /// An operation is considered to be ready if it doesn't have to block. Note that it is ready
    /// even when it will simply return an error because the channel is disconnected.
    ///
    /// The selected operation must be completed with [`SelectedOperation::send`]
    /// or [`SelectedOperation::recv`].
    ///
    /// [`SelectedOperation::send`]: struct.SelectedOperation.html#method.send
    /// [`SelectedOperation::recv`]: struct.SelectedOperation.html#method.recv
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
    /// let oper1 = sel.recv(&r1);
    /// let oper2 = sel.recv(&r2);
    ///
    /// // Both operations are initially ready, so a random one will be executed.
    /// let oper = sel.try_select();
    /// match oper {
    ///     Err(_) => panic!("both operations should be ready"),
    ///     Ok(oper) => match oper.index() {
    ///         i if i == oper1 => assert_eq!(oper.recv(&r1), Ok(10)),
    ///         i if i == oper2 => assert_eq!(oper.recv(&r2), Ok(20)),
    ///         _ => unreachable!(),
    ///     }
    /// }
    /// ```
    pub fn try_select(&mut self) -> Result<SelectedOperation<'_>, TrySelectError> {
        match run_select(&mut self.handles, Timeout::Now) {
            None => Err(TrySelectError),
            Some((token, index, ptr)) => Ok(SelectedOperation {
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
    /// An operation is considered to be ready if it doesn't have to block. Note that it is ready
    /// even when it will simply return an error because the channel is disconnected.
    ///
    /// The selected operation must be completed with [`SelectedOperation::send`]
    /// or [`SelectedOperation::recv`].
    ///
    /// [`SelectedOperation::send`]: struct.SelectedOperation.html#method.send
    /// [`SelectedOperation::recv`]: struct.SelectedOperation.html#method.recv
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
    /// let oper1 = sel.recv(&r1);
    /// let oper2 = sel.recv(&r2);
    ///
    /// // The second operation will be selected because it becomes ready first.
    /// let oper = sel.select();
    /// match oper.index() {
    ///     i if i == oper1 => assert_eq!(oper.recv(&r1), Ok(10)),
    ///     i if i == oper2 => assert_eq!(oper.recv(&r2), Ok(20)),
    ///     _ => unreachable!(),
    /// }
    /// ```
    pub fn select(&mut self) -> SelectedOperation<'_> {
        if self.handles.is_empty() {
            panic!("no operations have been added to `Select`");
        }

        let (token, index, ptr) = run_select(&mut self.handles, Timeout::Never).unwrap();
        SelectedOperation {
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
    /// An operation is considered to be ready if it doesn't have to block. Note that it is ready
    /// even when it will simply return an error because the channel is disconnected.
    ///
    /// The selected operation must be completed with [`SelectedOperation::send`]
    /// or [`SelectedOperation::recv`].
    ///
    /// [`SelectedOperation::send`]: struct.SelectedOperation.html#method.send
    /// [`SelectedOperation::recv`]: struct.SelectedOperation.html#method.recv
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
    /// let oper1 = sel.recv(&r1);
    /// let oper2 = sel.recv(&r2);
    ///
    /// // The second operation will be selected because it becomes ready first.
    /// let oper = sel.select_timeout(Duration::from_millis(500));
    /// match oper {
    ///     Err(_) => panic!("should not have timed out"),
    ///     Ok(oper) => match oper.index() {
    ///         i if i == oper1 => assert_eq!(oper.recv(&r1), Ok(10)),
    ///         i if i == oper2 => assert_eq!(oper.recv(&r2), Ok(20)),
    ///         _ => unreachable!(),
    ///     }
    /// }
    /// ```
    pub fn select_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<SelectedOperation<'_>, SelectTimeoutError> {
        let timeout = Timeout::At(Instant::now() + timeout);

        match run_select(&mut self.handles, timeout) {
            None => Err(SelectTimeoutError),
            Some((token, index, ptr)) => Ok(SelectedOperation {
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

/// A selected operation that needs to be completed.
///
/// To complete the operation, call [`send`] or [`recv`].
///
/// # Panics
///
/// Forgetting to complete the operation is an error and might lead to deadlocks. If a
/// `SelectedOperation` is dropped without completion, a panic occurs.
///
/// [`send`]: struct.SelectedOperation.html#method.send
/// [`recv`]: struct.SelectedOperation.html#method.recv
#[must_use]
pub struct SelectedOperation<'a> {
    /// Token needed to complete the operation.
    token: Token,

    /// The index of the selected operation.
    index: usize,

    /// The address of the selected `Sender` or `Receiver`.
    ptr: *const u8,

    /// Indicates that a `Select<'a>` is mutably borrowed.
    _marker: PhantomData<&'a mut Select<'a>>,
}

impl<'a> SelectedOperation<'a> {
    /// Returns the index of the selected operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{bounded, Select};
    ///
    /// let (s1, r1) = bounded::<()>(0);
    /// let (s2, r2) = bounded::<()>(0);
    /// let (s3, r3) = bounded::<()>(1);
    ///
    /// let mut sel = Select::new();
    /// let oper1 = sel.send(&s1);
    /// let oper2 = sel.recv(&r2);
    /// let oper3 = sel.send(&s3);
    ///
    /// // Only the last operation is ready.
    /// let oper = sel.select();
    /// assert_eq!(oper.index(), 2);
    /// assert_eq!(oper.index(), oper3);
    ///
    /// // Complete the operation.
    /// oper.send(&s3, ()).unwrap();
    /// ```
    pub fn index(&self) -> usize {
        self.index
    }

    /// Completes the send operation.
    ///
    /// The passed [`Sender`] reference must be the same one that was used in [`Select::send`]
    /// when the operation was added.
    ///
    /// # Panics
    ///
    /// Panics if an incorrect [`Sender`] reference is passed.
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
    /// let oper1 = sel.send(&s);
    ///
    /// let oper = sel.select();
    /// assert_eq!(oper.index(), oper1);
    /// assert_eq!(oper.send(&s, 10), Err(SendError(10)));
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
    /// The passed [`Receiver`] reference must be the same one that was used in [`Select::recv`]
    /// when the operation was added.
    ///
    /// # Panics
    ///
    /// Panics if an incorrect [`Receiver`] reference is passed.
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
    /// let oper1 = sel.recv(&r);
    ///
    /// let oper = sel.select();
    /// assert_eq!(oper.index(), oper1);
    /// assert_eq!(oper.recv(&r), Err(RecvError));
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

impl<'a> fmt::Debug for SelectedOperation<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("SelectedOperation").finish()
    }
}

impl<'a> Drop for SelectedOperation<'a> {
    fn drop(&mut self) {
        panic!("dropped `SelectedOperation` without completing the operation");
    }
}
