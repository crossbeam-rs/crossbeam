//! Interface to the select mechanism.

use std::marker::PhantomData;
use std::mem;
use std::option;
use std::ptr;
use std::sync::Arc;
use std::time::Instant;

use internal::channel::{self, Receiver, Sender};
use internal::context::{self, Context};
use internal::smallvec::SmallVec;
use internal::utils;

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
    /// Creates an identifier from a mutable reference.
    ///
    /// This function essentially just turns the address of the reference into a number. The
    /// reference should point to a variable that is specific to the thread and the operation,
    /// and is alive for the entire duration of select or blocking operation.
    #[inline]
    pub fn hook<T>(r: &mut T) -> Operation {
        Operation(r as *mut T as usize)
    }
}

/// Current state of a select or a blocking operation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Selected {
    /// Still waiting for an operation.
    Waiting,

    /// The select or blocking operation has been aborted.
    Aborted,

    /// A channel was closed.
    Closed,

    /// An operation became ready.
    Operation(Operation),
}

impl From<usize> for Selected {
    #[inline]
    fn from(val: usize) -> Selected {
        match val {
            0 => Selected::Waiting,
            1 => Selected::Aborted,
            2 => Selected::Closed,
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
            Selected::Closed => 2,
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
    fn register(&self, token: &mut Token, oper: Operation, cx: &Arc<Context>) -> bool;

    /// Unregisters the operation.
    fn unregister(&self, oper: Operation);

    /// Attempts to execute the selected operation.
    fn accept(&self, token: &mut Token, cx: &Arc<Context>) -> bool;

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

    fn register(&self, token: &mut Token, oper: Operation, cx: &Arc<Context>) -> bool {
        (**self).register(token, oper, cx)
    }

    fn unregister(&self, oper: Operation) {
        (**self).unregister(oper);
    }

    fn accept(&self, token: &mut Token, cx: &Arc<Context>) -> bool {
        (**self).accept(token, cx)
    }

    fn state(&self) -> usize {
        (**self).state()
    }
}

/// Runs until one of the operations is fired, potentially blocking the current thread.
///
/// Receive operations will have to be followed up by `read`, and send operations by `write`.
pub fn main_loop<S>(
    handles: &mut [(&S, usize, *const u8)],
    has_default: bool,
) -> (Token, usize, *const u8)
where
    S: SelectHandle + ?Sized,
{
    // Create a token, which serves as a temporary variable that gets initialized in this function
    // and is later used by a call to `read` or `write` that completes the selected operation.
    let mut token = Token::default();

    // Shuffle the operations for fairness.
    if handles.len() >= 2 {
        utils::shuffle(handles);
    }

    if handles.is_empty() {
        if has_default {
            // If there is only the `default` case, return.
            return (token, 0, ptr::null());
        } else {
            // If there are no operations at all, block forever.
            utils::sleep_forever();
        }
    }

    if has_default && handles.len() > 1 {
        let mut states = SmallVec::<[usize; 4]>::with_capacity(handles.len());

        // Snapshot the channel states of all operations.
        for &(handle, _, _) in handles.iter() {
            states.push(handle.state());
        }

        loop {
            // Try firing the operations.
            for &(handle, i, ptr) in handles.iter() {
                if handle.try(&mut token) {
                    return (token, i, ptr);
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
                return (token, 0, ptr::null());
            }
        }
    }

    loop {
        // Try firing the operations without blocking.
        for &(handle, i, ptr) in handles.iter() {
            if handle.try(&mut token) {
                return (token, i, ptr);
            }
        }

        if has_default {
            // Selected the `default` case.
            return (token, 0, ptr::null());
        }

        // Before blocking, try firing the operations one more time. Retries are permitted to take
        // a little bit more time than the initial tries, but they still mustn't block.
        for &(handle, i, ptr) in handles.iter() {
            if handle.retry(&mut token) {
                return (token, i, ptr);
            }
        }

        // Prepare for blocking.
        let res = context::with_current(|cx| {
            cx.reset();
            let mut sel = Selected::Waiting;
            let mut registered_count = 0;

            // Register all operations.
            for (handle, _, _) in handles.iter_mut() {
                registered_count += 1;

                // If registration returns `false`, that means the operation has just become ready.
                if !handle.register(&mut token, Operation::hook(handle), cx) {
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
                let mut deadline: Option<Instant> = None;
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
                handle.unregister(Operation::hook(handle));
            }

            match sel {
                Selected::Waiting => unreachable!(),
                Selected::Aborted => {},
                Selected::Closed | Selected::Operation(_) => {
                    // Find the selected operation.
                    for (handle, i, ptr) in handles.iter_mut() {
                        // Is this the selected operation?
                        if sel == Selected::Operation(Operation::hook(handle)) {
                            // Try firing this operation.
                            if handle.accept(&mut token, cx) {
                                return Some((*i, *ptr));
                            }
                        }
                    }

                    // Before the next round, reshuffle the operations for fairness.
                    if handles.len() >= 2 {
                        utils::shuffle(handles);
                    }
                },
            }

            None
        });

        // Return if an operation was fired.
        if let Some((i, ptr)) = res {
            return (token, i, ptr);
        }
    }
}

/// Waits on a set of channel operations.
///
/// This struct with builder-like interface allows declaring a set of channel operations and
/// blocking until any one of them becomes ready. Finally, one of the operations is executed. If
/// multiple operations are ready at the same time, a random one is chosen. It is also possible to
/// declare a default case that gets executed if none of the operations are initially ready.
///
/// TODO: when to use this and when to use select!? Also mention in the docs for select!.
///
/// # Receiving
///
/// Receiving a message from two channels, whichever becomes ready first:
///
/// ```
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// thread::spawn(move || s1.send("foo"));
/// thread::spawn(move || s2.send("bar"));
///
/// // Only one of these two receive operations will be executed.
/// channel::Select::new()
///     .recv(&r1, |msg| assert_eq!(msg, Some("foo")))
///     .recv(&r2, |msg| assert_eq!(msg, Some("bar")))
///     .wait();
/// ```
///
/// # Sending
///
/// Waiting on a send and a receive operation:
///
/// ```
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// s1.send("foo");
///
/// // Since both operations are initially ready, a random one will be executed.
/// channel::Select::new()
///     .recv(&r1, |msg| assert_eq!(msg, Some("foo")))
///     .send(&s2, || "bar", || assert_eq!(r2.recv(), Some("bar")))
///     .wait();
/// ```
///
/// # Default case
///
/// A special kind of case is `default`, which gets executed if none of the operations can be
/// executed, i.e. they would block:
///
/// ```
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::unbounded();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     s.send("foo");
/// });
///
/// // Don't block on the receive operation.
/// channel::Select::new()
///     .recv(&r, |_| panic!())
///     .default(|| println!("The message is not yet available."))
///     .wait();
/// ```
///
/// # Execution
///
/// 1. A `Select` is constructed, cases are added, and `.wait()` is called.
/// 2. If any of the `recv` or `send` operations are ready, one of them is executed. If multiple
///    operations are ready, a random one is chosen.
/// 3. If none of the `recv` and `send` operations are ready, the `default` case is executed. If
///    there is no `default` case, the current thread is blocked until an operation becomes ready.
/// 4. If a `recv` operation gets executed, its callback is invoked.
/// 5. If a `send` operation gets executed, the message is lazily evaluated and sent into the
///    channel. Finally, the callback is invoked.
///
/// **Note**: If evaluation of the message panics, the process will be aborted because it's
/// impossible to recover from such panics. All the other callbacks are allowed to panic, however.
#[must_use]
pub struct Select<'a, R> {
    /// A list of senders and receivers participating in selection.
    handles: SmallVec<[(&'a SelectHandle, usize, *const u8); 4]>,

    /// A list of callbacks, one per handle.
    callbacks: SmallVec<[Callback<'a, R>; 4]>,

    /// Callback for the default case.
    default: Option<Callback<'a, R>>,
}

impl<'a, R> Select<'a, R> {
    /// Creates a new `Select`.
    pub fn new() -> Select<'a, R> {
        Select {
            handles: SmallVec::new(),
            callbacks: SmallVec::new(),
            default: None,
        }
    }

    /// Adds a receive case.
    ///
    /// The callback will get invoked if the receive operation completes.
    #[inline]
    pub fn recv<T, C>(mut self, r: &'a Receiver<T>, cb: C) -> Select<'a, R>
    where
        C: FnOnce(Option<T>) -> R + 'a,
    {
        let i = self.handles.len() + 1;
        let ptr = r as *const Receiver<_> as *const u8;
        self.handles.push((r, i, ptr));

        self.callbacks.push(Callback::new(move |token| {
            let msg = unsafe { channel::read(r, token) };
            cb(msg)
        }));

        self
    }

    /// Adds a send case.
    ///
    /// If the send operation succeeds, the message will be generated and sent into the channel.
    /// Finally, the callback gets invoked once the operation is completed.
    ///
    /// **Note**: If function `msg` panics, the process will be aborted because it's impossible to
    /// recover from such panics. However, function `cb` is allowed to panic.
    #[inline]
    pub fn send<T, M, C>(mut self, s: &'a Sender<T>, msg: M, cb: C) -> Select<'a, R>
    where
        M: FnOnce() -> T + 'a,
        C: FnOnce() -> R + 'a,
    {
        let i = self.handles.len() + 1;
        let ptr = s as *const Sender<_> as *const u8;
        self.handles.push((s, i, ptr));

        self.callbacks.push(Callback::new(move |token| {
            let _guard = utils::AbortGuard(
                "a send case triggered a panic while evaluating its message"
            );
            let msg = msg();

            ::std::mem::forget(_guard);
            unsafe { channel::write(s, token, msg); }

            cb()
        }));

        self
    }

    /// Adds a default case.
    ///
    /// This case gets executed if none of the channel operations are ready.
    ///
    /// If called more than once, this method keeps only the last callback for the default case.
    #[inline]
    pub fn default<C>(mut self, cb: C) -> Select<'a, R>
    where
        C: FnOnce() -> R + 'a,
    {
        self.default = Some(Callback::new(move |_| cb()));
        self
    }

    /// Starts selection and waits until it completes.
    ///
    /// The result of the executed callback function will be returned.
    pub fn wait(mut self) -> R {
        let (mut token, index, _) = main_loop(&mut self.handles, self.default.is_some());
        let cb;

        if index == 0 {
            self.callbacks.clear();
            cb = self.default.take().unwrap();
        } else {
            cb = self.callbacks.remove(index - 1);
            self.callbacks.clear();
            self.default.take();
        }

        cb.call(&mut token)
    }
}

/// Some space to keep a `FnOnce()` object on the stack.
type Space = [usize; 2];

/// A `FnOnce(&mut Token) -> R + 'a` that is stored inline if small, or otherwise boxed on the heap.
pub struct Callback<'a, R> {
    call: unsafe fn(*mut u8, token: Option<&mut Token>) -> Option<R>,
    space: Space,
    _marker: PhantomData<(*mut (), &'a ())>, // !Send + !Sync
}

impl<'a, R> Callback<'a, R> {
    /// Constructs a new `Callback<'a, R>` from a `FnOnce(&mut Token) -> R + 'a`.
    pub fn new<F>(f: F) -> Self
    where
        F: FnOnce(&mut Token) -> R + 'a,
    {
        let size = mem::size_of::<F>();
        let align = mem::align_of::<F>();

        unsafe {
            if size <= mem::size_of::<Space>() && align <= mem::align_of::<Space>() {
                let mut space: Space = mem::uninitialized();
                ptr::write(&mut space as *mut Space as *mut F, f);

                unsafe fn call<'a, F, R>(raw: *mut u8, token: Option<&mut Token>) -> Option<R>
                where
                    F: FnOnce(&mut Token) -> R + 'a,
                {
                    let f: F = ptr::read(raw as *mut F);
                    token.map(f)
                }

                Callback {
                    call: call::<F, R>,
                    space,
                    _marker: PhantomData,
                }
            } else {
                let b: Box<F> = Box::new(f);
                let mut space: Space = mem::uninitialized();
                ptr::write(&mut space as *mut Space as *mut Box<F>, b);

                unsafe fn call<'a, F, R>(raw: *mut u8, token: Option<&mut Token>) -> Option<R>
                where
                    F: FnOnce(&mut Token) -> R + 'a,
                {
                    let b: Box<F> = ptr::read(raw as *mut Box<F>);
                    token.map(*b)
                }

                Callback {
                    call: call::<F, R>,
                    space,
                    _marker: PhantomData,
                }
            }
        }
    }

    /// Invokes the callback.
    #[inline]
    pub fn call(mut self, token: &mut Token) -> R {
        let res = unsafe {
            (self.call)(&mut self.space as *mut Space as *mut u8, Some(token))
        };
        mem::forget(self);
        res.unwrap()
    }
}

impl<'a, R> Drop for Callback<'a, R> {
    fn drop(&mut self) {
        unsafe {
            (self.call)(&mut self.space as *mut Space as *mut u8, None);
        }
    }
}

/// Dereference the pointer and bind it to the lifetime in the iterator.
///
/// The returned reference will appear as if it was previously produced by the iterator.
pub unsafe fn deref_from_iterator<'a, T: 'a, I>(ptr: *const T, _: &I) -> &'a T
where
    I: Iterator<Item = &'a T>,
{
    &*ptr
}

/// Receiver argument types allowed in `recv` cases.
pub trait RecvArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Receiver<T>>;

    /// Converts the argument into an iterator over receivers.
    fn __as_recv_argument(&'a self) -> Self::Iter;
}

impl<'a, T: 'a> RecvArgument<'a, T> for Receiver<T> {
    type Iter = option::IntoIter<&'a Receiver<T>>;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        Some(self).into_iter()
    }
}

impl<'a, T: 'a> RecvArgument<'a, T> for &'a Receiver<T> {
    type Iter = option::IntoIter<&'a Receiver<T>>;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a> RecvArgument<'a, T> for &'a &'a Receiver<T> {
    type Iter = option::IntoIter<&'a Receiver<T>>;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        Some(**self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Receiver<T>> + Clone> RecvArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

/// Sender argument types allowed in `send` cases.
pub trait SendArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Sender<T>>;

    /// Converts the argument into an iterator over senders.
    fn __as_send_argument(&'a self) -> Self::Iter;
}

impl<'a, T: 'a> SendArgument<'a, T> for Sender<T> {
    type Iter = option::IntoIter<&'a Sender<T>>;

    fn __as_send_argument(&'a self) -> Self::Iter {
        Some(self).into_iter()
    }
}

impl<'a, T: 'a> SendArgument<'a, T> for &'a Sender<T> {
    type Iter = option::IntoIter<&'a Sender<T>>;

    fn __as_send_argument(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a> SendArgument<'a, T> for &'a &'a Sender<T> {
    type Iter = option::IntoIter<&'a Sender<T>>;

    fn __as_send_argument(&'a self) -> Self::Iter {
        Some(**self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Sender<T>> + Clone> SendArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;

    fn __as_send_argument(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

/// Waits on a set of channel operations.
///
/// This macro allows declaring a set of channel operations and blocking until any one of them
/// becomes ready. Finally, one of the operations is executed. If multiple operations are ready at
/// the same time, a random one is chosen. It is also possible to declare a `default` case that
/// gets executed if none of the operations are initially ready.
///
/// # Receiving
///
/// Receiving a message from two channels, whichever becomes ready first:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// thread::spawn(move || s1.send("foo"));
/// thread::spawn(move || s2.send("bar"));
///
/// // Only one of these two receive operations will be executed.
/// select! {
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
///     recv(r2, msg) => assert_eq!(msg, Some("bar")),
/// }
/// # }
/// ```
///
/// # Sending
///
/// Waiting on a send and a receive operation:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// s1.send("foo");
///
/// // Since both operations are initially ready, a random one will be executed.
/// select! {
///     recv(r1, msg) => assert_eq!(msg, Some("foo")),
///     send(s2, "bar") => assert_eq!(r2.recv(), Some("bar")),
/// }
/// # }
/// ```
///
/// # Default case
///
/// A special kind of case is `default`, which gets executed if none of the operations can be
/// executed, i.e. they would block:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// let (s, r) = channel::unbounded();
///
/// thread::spawn(move || {
///     thread::sleep(Duration::from_secs(1));
///     s.send("foo");
/// });
///
/// // Don't block on the receive operation.
/// select! {
///     recv(r) => panic!(),
///     default => println!("The message is not yet available."),
/// }
/// # }
/// ```
///
/// # Iterators
///
/// It is possible to have arbitrary iterators of senders or receivers in a single `send` or `recv`
/// case:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel;
/// # fn main() {
/// use std::thread;
/// use std::time::{Duration, Instant};
/// use crossbeam_channel as channel;
///
/// let (s1, r1) = channel::unbounded();
/// let (s2, r2) = channel::unbounded();
///
/// s1.send("foo");
/// s2.send("bar");
/// let receivers = vec![r1, r2];
///
/// // Both receivers are initially ready so one of the two receive operations
/// // will be chosen randomly.
/// select! {
///     // The third argument to `recv` is optional and is assigned a
///     // reference to the receiver the message was received from.
///     recv(receivers, msg, from) => {
///         for (i, r) in receivers.iter().enumerate() {
///             if r == from {
///                 println!("Received {:?} from the {}-th receiver.", msg, i);
///             }
///         }
///     }
/// }
/// # }
/// ```
///
/// # Syntax
///
/// An invocation of `select!` consists of a list of cases. Consecutive cases are delimited by a
/// comma, but it's not required if the preceding case has a block expression (the syntax is very
/// similar to `match` statements).
///
/// The following invocation illustrates all the possible forms cases can take:
///
/// ```ignore
/// select! {
///     recv(r1) => body1,
///     recv(r2, msg2) => body2,
///     recv(r3, msg3, from3) => body3,
///
///     send(s4, msg4) => body4,
///     send(s5, msg5, into5) => body5,
///
///     default => body6,
/// }
/// ```
///
/// Input expressions: `r1`, `r2`, `r3`, `s4`, `s5`, `msg4`, `msg5`, `body1`, `body2`, `body3`,
/// `body4`, `body5`, `body6`
///
/// Output patterns: `msg2`, `msg3`, `msg4`, `msg5`, `from3`, `into5`
///
/// Types of expressions and patterns (generic over types `A`, `B`, `C`, `D`, `E`, and `F`):
///
/// * `r1`: one of `Receiver<A>`, `&Receiver<A>`, or `impl IntoIterator<Item = &Receiver<A>>`
/// * `r2`: one of `Receiver<B>`, `&Receiver<B>`, or `impl IntoIterator<Item = &Receiver<B>>`
/// * `r3`: one of `Receiver<C>`, `&Receiver<C>`, or `impl IntoIterator<Item = &Receiver<C>>`
/// * `s4`: one of `Sender<D>`, `&Sender<D>`, or `impl IntoIterator<Item = &Sender<D>>`
/// * `s5`: one of `Sender<E>`, `&Sender<E>`, or `impl IntoIterator<Item = &Sender<E>>`
/// * `msg2`: `Option<B>`
/// * `msg3`: `Option<C>`
/// * `msg4`: `D`
/// * `msg5`: `E`
/// * `from3`: `&Receiver<C>`
/// * `into5`: `&Sender<E>`
/// * `body1`, `body2`, `body3`, `body4`, `body5`, `body6`: `F`
///
/// Pattern `from3` is bound to the receiver in `r3` from which `msg3` was received.
///
/// Pattern `into5` is bound to the sender in `s5` into which `msg5` was sent.
///
/// There can be at most one `default` case.
///
/// # Execution
///
/// 1. All sender and receiver arguments (`r1`, `r2`, `r3`, `s4`, and `s5`) are evaluated.
/// 2. If any of the `recv` or `send` operations are ready, one of them is executed. If multiple
///    operations are ready, a random one is chosen.
/// 3. If none of the `recv` and `send` operations are ready, the `default` case is executed. If
///    there is no `default` case, the current thread is blocked until an operation becomes ready.
/// 4. If a `recv` operation gets executed, the message pattern (`msg2` or `msg3`) is
///    bound to the received message, and the receiver pattern (`from3`) is bound to the receiver
///    from which the message was received.
/// 5. If a `send` operation gets executed, the message (`msg4` or `msg5`) is evaluated and sent
///    into the channel. Then, the sender pattern (`into5`) is bound to the sender into which the
///    message was sent.
/// 6. Finally, the body (`body1`, `body2`, `body3`, `body4`, `body5`, or `body6`) of the executed
///    case is evaluated. The whole `select!` invocation evaluates to that expression.
///
/// **Note**: If evaluation of `msg4` or `msg5` panics, the process will be aborted because it's
/// impossible to recover from such panics. All the other expressions are allowed to panic,
/// however.
#[macro_export]
macro_rules! select {
    // The macro consists of two stages:
    // 1. Parsing
    // 2. Code generation
    //
    // The parsing stage consists of these subparts:
    // 1. parse_list: Turns a list of tokens into a list of cases.
    // 2. parse_list_error: Diagnoses the syntax error.
    // 3. parse_case: Parses a single case and verifies its argument list.
    //
    // The codegen stage consists of these subparts:
    // 1. codegen_fast_path: Optimizes `select!` into a single send or receive operation.
    // 2. codegen_main_loop: Builds the main loop that fires cases and puts the thread to sleep.
    // 3. codegen_container: Initializes the vector containing channel operations.
    // 4: codegen_push: Pushes an operation into the vector of operations.
    // 5. codegen_has_default: A helper that checks whether there's a default operation.
    // 6. codegen_finalize: Completes the channel operation that has been selected.
    //
    // If the parsing stage encounters a syntax error, it fails with a compile-time error.
    // Otherwise, the macro parses the input into three token trees and passes them to the code
    // generation stage. The three token trees are lists of comma-separated cases, written inside
    // parentheses:
    // 1. Receive cases.
    // 2. Send cases.
    // 3. Default cases (there can be at most one).
    //
    // Each case is of the form `(index, variable) case(arguments) => block`, where:
    // - `index` is a unique index for the case (index 0 is reserved for the `default` case).
    // - `variable` is a unique variable name associated with it.
    // - `case` is one of `recv`, `send`, or `default`.
    // - `arguments` is a list of arguments.
    //
    // All lists, if not empty, have a trailing comma at the end.
    //
    // For example, this invocation of `select!`:
    //
    // ```ignore
    // select! {
    //     recv(a) => x,
    //     recv(b, m) => y,
    //     send(s, msg) => { z }
    //     default => {}
    // }
    // ```
    //
    // Would be parsed as:
    //
    // ```ignore
    // ((1usize case1) recv(a, _, _) => { x }, (2usize, case2) recv(b, m, _) => { y },)
    // ((3usize case3) send(s, msg, _) => { { z } },)
    // ((0usize case0) default() => { {} },)
    // ```
    //
    // These three lists are then passed to the code generation stage.

    // Success! The list is empty.
    // Now check the arguments of each processed case.
    (@parse_list
        ($($head:tt)*)
        ()
    ) => {
        select!(
            @parse_case
            ()
            ()
            ()
            ($($head)*)
            (
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
        )
    };
    // If necessary, insert an empty argument list after `default`.
    (@parse_list
        ($($head:tt)*)
        (default => $($tail:tt)*)
    ) => {
        select!(
            @parse_list
            ($($head)*)
            (default() => $($tail)*)
        )
    };
    // The first case is separated by a comma.
    (@parse_list
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr, $($tail:tt)*)
    ) => {
        select!(
            @parse_list
            ($($head)* $case $args => { $body },)
            ($($tail)*)
        )
    };
    // Print an error if there is a semicolon after the block.
    (@parse_list
        ($($head:tt)*)
        ($case:ident $args:tt => $body:block; $($tail:tt)*)
    ) => {
        compile_error!("did you mean to put a comma instead of the semicolon after `}`?")
    };
    // Don't require a comma after the case if it has a proper block.
    (@parse_list
        ($($head:tt)*)
        ($case:ident $args:tt => $body:block $($tail:tt)*)
    ) => {
        select!(
            @parse_list
            ($($head)* $case $args => { $body },)
            ($($tail)*)
        )
    };
    // Only one case remains.
    (@parse_list
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr)
    ) => {
        select!(
            @parse_list
            ($($head)* $case $args => { $body },)
            ()
        )
    };
    // Accept a trailing comma at the end of the list.
    (@parse_list
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr,)
    ) => {
        select!(
            @parse_list
            ($($head)* $case $args => { $body },)
            ()
        )
    };
    // Diagnose and print an error.
    (@parse_list
        ($($head:tt)*)
        ($($tail:tt)*)
    ) => {
        select!(@parse_list_error1 $($tail)*)
    };
    // Stage 1: check the case type.
    (@parse_list_error1 recv $($tail:tt)*) => {
        select!(@parse_list_error2 recv $($tail)*)
    };
    (@parse_list_error1 send $($tail:tt)*) => {
        select!(@parse_list_error2 send $($tail)*)
    };
    (@parse_list_error1 default $($tail:tt)*) => {
        select!(@parse_list_error2 default $($tail)*)
    };
    (@parse_list_error1 $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected one of `recv`, `send`, or `default`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@parse_list_error1 $($tail:tt)*) => {
        select!(@parse_list_error2 $($tail)*);
    };
    // Stage 2: check the argument list.
    (@parse_list_error2 $case:ident) => {
        compile_error!(concat!(
            "missing argument list after `",
            stringify!($case),
            "`",
        ))
    };
    (@parse_list_error2 $case:ident => $($tail:tt)*) => {
        compile_error!(concat!(
            "missing argument list after `",
            stringify!($case),
            "`",
        ))
    };
    (@parse_list_error2 $($tail:tt)*) => {
        select!(@parse_list_error3 $($tail)*)
    };
    // Stage 3: check the `=>` and what comes after it.
    (@parse_list_error3 $case:ident($($args:tt)*)) => {
        compile_error!(concat!(
            "missing `=>` after the argument list of `",
            stringify!($case),
            "`",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) =>) => {
        compile_error!("expected expression after `=>`")
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => $body:expr; $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma instead of the semicolon after `",
            stringify!($body),
            "`?",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => recv($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => send($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => default($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => $f:ident($($a:tt)*) $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "(",
            stringify!($($a)*),
            ")`?",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => $f:ident!($($a:tt)*) $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "!(",
            stringify!($($a)*),
            ")`?",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => $f:ident![$($a:tt)*] $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "![",
            stringify!($($a)*),
            "]`?",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => $f:ident!{$($a:tt)*} $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "!{",
            stringify!($($a)*),
            "}`?",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) => $body:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($body),
            "`?",
        ))
    };
    (@parse_list_error3 $case:ident($($args:tt)*) $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected `=>`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@parse_list_error3 $case:ident $args:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected an argument list, found `",
            stringify!($args),
            "`",
        ))
    };
    (@parse_list_error3 $($tail:tt)*) => {
        select!(@parse_list_error4 $($tail)*)
    };
    // Stage 4: fail with a generic error message.
    (@parse_list_error4 $($tail:tt)*) => {
        compile_error!("invalid syntax")
    };

    // Success! All cases were parsed.
    (@parse_case
        ($($recv:tt)*)
        ($($send:tt)*)
        $default:tt
        ()
        $labels:tt
    ) => {
        select!(
            @codegen_declare
            ($($recv)* $($send)*)
            ($($recv)*)
            ($($send)*)
            $default
        )
    };
    // Error: there are no labels left.
    (@parse_case
        $recv:tt
        $send:tt
        $default:tt
        $cases:tt
        ()
    ) => {
        compile_error!("too many cases in a `select!` block")
    };
    // Check the format of a `recv` case...
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            ($($recv)* $label recv(&$r, _, _) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            ($($recv)* $label recv(&$r, $m, _) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            ($($recv)* $label recv($rs, $m, $r) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Allow trailing comma...
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            ($($recv)* $label recv($r, _, _) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr, $m:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            ($($recv)* $label recv($r, $m, _) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($rs:expr, $m:pat, $r:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            ($($recv)* $label recv($rs, $m, $r) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Error cases...
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($($args:tt)*) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "invalid argument list in `recv(",
            stringify!($($args)*),
            ")`",
        ))
    };
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv $t:tt => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "expected an argument list after `recv`, found `",
            stringify!($t),
            "`",
        ))
    };

    // Check the format of a `send` case...
    (@parse_case
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($s:expr, $m:expr) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            $recv
            ($($send)* $label send($s, $m, _) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            $recv
            ($($send)* $label send($ss, $m, $s) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Allow trailing comma...
    (@parse_case
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($s:expr, $m:expr,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            $recv
            ($($send)* $label send($s, $m, _) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($ss:expr, $m:expr, $s:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
            @parse_case
            $recv
            ($($send)* $label send($ss, $m, $s) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Error cases...
    (@parse_case
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($($args:tt)*) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "invalid argument list in `send(",
            stringify!($($args)*),
            ")`",
        ))
    };
    (@parse_case
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send $args:tt => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "expected an argument list after `send`, found `",
            stringify!($args),
            "`",
        ))
    };

    // Check the format of a `default` case.
    (@parse_case
        $recv:tt
        $send:tt
        ()
        (default() => $body:tt, $($tail:tt)*)
        ($($labels:tt)*)
    ) => {
        select!(
            @parse_case
            $recv
            $send
            ((0usize case0) default() => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Valid, but duplicate default cases...
    (@parse_case
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default() => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    // Other error cases...
    (@parse_case
        $recv:tt
        $send:tt
        $default:tt
        (default($($args:tt)*) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "invalid argument list in `default(",
            stringify!($($args)*),
            ")`",
        ))
    };
    (@parse_case
        $recv:tt
        $send:tt
        $default:tt
        (default $t:tt => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "expected an argument list after `default`, found `",
            stringify!($t),
            "`",
        ))
    };

    // The case was not consumed, therefore it must be invalid.
    (@parse_case
        $recv:tt
        $send:tt
        $default:tt
        ($case:ident $args:tt => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!(concat!(
            "expected one of `recv`, `send`, or `default`, found `",
            stringify!($case),
            "`",
        ))
    };

    // Declare the iterator variable for a `recv` case.
    (@codegen_declare
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        match {
            #[allow(unused_imports)]
            use $crate::internal::select::RecvArgument;
            &mut (&&$rs).__as_recv_argument()
        } {
            $var => {
                select!(
                    @codegen_declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    }};
    // Declare the iterator variable for a `send` case.
    (@codegen_declare
        (($i:tt $var:ident) send($ss:expr, $m:pat, $s:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        match {
            #[allow(unused_imports)]
            use $crate::internal::select::SendArgument;
            &mut (&&$ss).__as_send_argument()
        } {
            $var => {
                select!(
                    @codegen_declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    }};
    // All iterator variables have been declared.
    (@codegen_declare
        ()
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        #[cfg_attr(feature = "cargo-clippy", allow(clippy))]
        let mut handles = select!(@codegen_container $recv $send);
        select!(@codegen_fast_path $recv $send $default handles)
    }};

    // Attempt to optimize the whole `select!` into a single call to `recv`.
    (@codegen_fast_path
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
        ()
        $handles:ident
    ) => {{
        if $handles.len() == 1 {
            let $r = $handles[0].0;
            let $m = $handles[0].0.recv();

            drop($handles);
            $body
        } else {
            select!(
                @codegen_main_loop
                (($i $var) recv($rs, $m, $r) => $body,)
                ()
                ()
                $handles
            )
        }
    }};
    // Attempt to optimize the whole `select!` into a single call to `recv_nonblocking`.
    (@codegen_fast_path
        (($recv_i:tt $recv_var:ident) recv($rs:expr, $m:pat, $r:pat) => $recv_body:tt,)
        ()
        (($default_i:tt $default_var:ident) default() => $default_body:tt,)
        $handles:ident
    ) => {{
        if $handles.len() == 1 {
            let r = $handles[0].0;
            let msg;

            match $crate::internal::channel::recv_nonblocking(r) {
                $crate::internal::channel::RecvNonblocking::Message(m) => {
                    msg = Some(m);
                    let $m = msg;
                    let $r = r;

                    drop($handles);
                    $recv_body
                }
                $crate::internal::channel::RecvNonblocking::Closed => {
                    msg = None;
                    let $m = msg;
                    let $r = r;

                    drop($handles);
                    $recv_body
                }
                $crate::internal::channel::RecvNonblocking::Empty => {
                    drop($handles);
                    $default_body
                }
            }
        } else {
            select!(
                @codegen_main_loop
                (($recv_i $recv_var) recv($rs, $m, $r) => $recv_body,)
                ()
                (($default_i $default_var) default() => $default_body,)
                $handles
            )
        }
    }};
    // Move on to the main select loop.
    (@codegen_fast_path
        $recv:tt
        $send:tt
        $default:tt
        $handles:ident
    ) => {{
        select!(
            @codegen_main_loop
            $recv
            $send
            $default
            $handles
        )
    }};
    // TODO: Optimize `select! { send(s, msg) => {} }`.
    // TODO: Optimize `select! { send(s, msg) => {} default => {} }`.

    // The main select loop.
    (@codegen_main_loop
        $recv:tt
        $send:tt
        $default:tt
        $handles:ident
    ) => {{
        // Check if there's a `default` case.
        let has_default = select!(@codegen_has_default $default);

        // Run the main loop.
        #[allow(unused_mut)]
        #[allow(unused_variables)]
        let (mut token, index, selected) = $crate::internal::select::main_loop(
            &mut $handles,
            has_default,
        );

        // Pass the result of the main loop to the final step.
        select!(
            @codegen_finalize
            token
            index
            selected
            $handles
            $recv
            $send
            $default
        )
    }};

    // Initialize the `handles` vector if there's only a single `recv` case.
    (@codegen_container
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::Receiver<_>, usize, *const u8); 4]
        >::new();
        while let Some(r) = $var.next() {
            let ptr = r as *const $crate::Receiver<_> as *const u8;
            c.push((r, $i, ptr));
        }
        c
    }};
    // Initialize the `handles` vector if there's only a single `send` case.
    (@codegen_container
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt,)
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::Sender<_>, usize, *const u8); 4]
        >::new();
        while let Some(s) = $var.next() {
            let ptr = s as *const $crate::Sender<_> as *const u8;
            c.push((s, $i, ptr));
        }
        c
    }};
    // Initialize the `handles` vector generically.
    (@codegen_container
        $recv:tt
        $send:tt
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::internal::select::SelectHandle, usize, *const u8); 4]
        >::new();
        select!(@codegen_push c $recv $send);
        c
    }};

    // Push a `recv` operation into the `handles` vector.
    (@codegen_push
        $handles:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        while let Some(r) = $var.next() {
            let ptr = r as *const $crate::Receiver<_> as *const u8;
            $handles.push((r, $i, ptr));
        }
        select!(@codegen_push $handles ($($tail)*) $send);
    };
    // Push a `send` operation into the `handles` vector.
    (@codegen_push
        $handles:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        while let Some(s) = $var.next() {
            let ptr = s as *const $crate::Sender<_> as *const u8;
            $handles.push((s, $i, ptr));
        }
        select!(@codegen_push $handles () ($($tail)*));
    };
    // There are no more operations to push.
    (@codegen_push
        $handles:ident
        ()
        ()
    ) => {
    };

    // Evaluate to `false` if there is no `default` case.
    (@codegen_has_default
        ()
    ) => {
        false
    };
    // Evaluate to `true` if there is a `default` case.
    (@codegen_has_default
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        true
    };

    // Finalize a receive operation.
    (@codegen_finalize
        $token:ident
        $index:ident
        $selected:ident
        $handles:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
        $default:tt
    ) => {
        if $index == $i {
            #[allow(unsafe_code)]
            let ($m, $r) = unsafe {
                #[cfg_attr(feature = "cargo-clippy", allow(clippy))]
                let r = $crate::internal::select::deref_from_iterator(
                    $selected as *const $crate::Receiver<_>,
                    &$var,
                );
                let msg = $crate::internal::channel::read(r, &mut $token);
                (msg, r)
            };

            drop($handles);
            $body
        } else {
            select!(
                @codegen_finalize
                $token
                $index
                $selected
                $handles
                ($($tail)*)
                $send
                $default
            )
        }
    };
    // Finalize a send operation.
    (@codegen_finalize
        $token:ident
        $index:ident
        $selected:ident
        $handles:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
        $default:tt
    ) => {
        if $index == $i {
            let $s = {
                // We have to prefix variables with an underscore to get rid of warnings when
                // evaluation of `$m` doesn't finish.
                #[allow(unsafe_code)]
                let _s = unsafe {
                    #[cfg_attr(feature = "cargo-clippy", allow(clippy))]
                    $crate::internal::select::deref_from_iterator(
                        $selected as *const $crate::Sender<_>,
                        &$var,
                    )
                };
                let _guard = $crate::internal::utils::AbortGuard(
                    "a send case triggered a panic while evaluating its message"
                );
                let _msg = $m;

                #[allow(unreachable_code)]
                {
                    ::std::mem::forget(_guard);
                    #[allow(unsafe_code)]
                    unsafe { $crate::internal::channel::write(_s, &mut $token, _msg); }
                    _s
                }
            };

            drop($handles);
            $body
        } else {
            select!(
                @codegen_finalize
                $token
                $index
                $selected
                $handles
                ()
                ($($tail)*)
                $default
            )
        }
    };
    // Execute the default case.
    (@codegen_finalize
        $token:ident
        $index:ident
        $selected:ident
        $handles:ident
        ()
        ()
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        if $index == $i {
            drop($handles);
            $body
        } else {
            select!(
                @codegen_finalize
                $token
                $index
                $selected
                $handles
                ()
                ()
                ()
            )
        }
    };
    // No more cases to finalize.
    (@codegen_finalize
        $token:ident
        $index:ident
        $selected:ident
        $handles:ident
        ()
        ()
        ()
    ) => {
        unreachable!("internal error in crossbeam-channel")
    };

    // Catches a bug within this macro (should not happen).
    (@$($tokens:tt)*) => {
        compile_error!(concat!(
            "internal error in crossbeam-channel: ",
            stringify!(@$($tokens)*),
        ))
    };

    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        select!(
            @parse_list
            ()
            ($($case $(($($args)*))* => $body,)*)
        )
    };

    ($($tokens:tt)*) => {
        select!(
            @parse_list
            ()
            ($($tokens)*)
        )
    };
}
