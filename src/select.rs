//! Interface to the select mechanism.

use std::marker::PhantomData;
use std::time::{Duration, Instant};

use channel::{self, Receiver, Sender};
use context::Context;
use err::{RecvError, SendError};
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
    /// Creates an identifier from a mutable reference.
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

    /// The select or blocking operation has been aborted.
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

#[derive(Eq, PartialEq)]
enum Timeout {
    Now,
    Never,
    At(Instant),
}

/// Runs until one of the operations is fired, potentially blocking the current thread.
///
/// Receive operations will have to be followed up by `read`, and send operations by `write`.
fn main_loop<S>(
    handles: &mut [(&S, usize, *const u8)],
    timeout: Timeout,
) -> Option<(Token, usize, *const u8)>
where
    S: SelectHandle + ?Sized,
{
    // Create a token, which serves as a temporary variable that gets initialized in this function
    // and is later used by a call to `read` or `write` that completes the selected operation.
    let mut token = Token::default();

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

    // TODO: rename and extract this
    let try_select = |handles: &mut [(&S, usize, *const u8)], mut token: Token| {
        if handles.len() <= 1 {
            // Try firing the operations without blocking.
            for &(handle, i, ptr) in handles.iter() {
                if handle.try(&mut token) {
                    return Some((token, i, ptr));
                }
            }

            None
        } else {
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
    };

    if timeout == Timeout::Now {
        return try_select(handles, token);
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
                    return try_select(handles, token);
                }
            }
        };
    }
}

// /// Waits on a set of channel operations.
// ///
// /// This struct with builder-like interface allows declaring a set of channel operations and
// /// blocking until any one of them becomes ready. Finally, one of the operations is executed. If
// /// multiple operations are ready at the same time, a random one is chosen. It is also possible to
// /// declare a default case that gets executed if none of the operations are initially ready.
// ///
// /// Note that this method of selecting over channel operations is typically somewhat slower than
// /// the [`select!`] macro.
// ///
// /// [`select!`]: macro.select.html
// ///
// /// # Receiving
// ///
// /// Receiving a message from two channels, whichever becomes ready first:
// ///
// /// ```
// /// use std::thread;
// /// use crossbeam_channel as channel;
// ///
// /// let (s1, r1) = channel::unbounded();
// /// let (s2, r2) = channel::unbounded();
// ///
// /// thread::spawn(move || s1.send("foo"));
// /// thread::spawn(move || s2.send("bar"));
// ///
// /// // Only one of these two receive operations will be executed.
// /// channel::Select::new()
// ///     .recv(&r1, |msg| assert_eq!(msg, Some("foo")))
// ///     .recv(&r2, |msg| assert_eq!(msg, Some("bar")))
// ///     .wait();
// /// ```
// ///
// /// # Sending
// ///
// /// Waiting on a send and a receive operation:
// ///
// /// ```
// /// use std::thread;
// /// use crossbeam_channel as channel;
// ///
// /// let (s1, r1) = channel::unbounded();
// /// let (s2, r2) = channel::unbounded();
// ///
// /// s1.send("foo");
// ///
// /// // Since both operations are initially ready, a random one will be executed.
// /// channel::Select::new()
// ///     .recv(&r1, |msg| assert_eq!(msg, Some("foo")))
// ///     .send(&s2, || "bar", || assert_eq!(r2.recv(), Some("bar")))
// ///     .wait();
// /// ```
// ///
// /// # Default case
// ///
// /// A special kind of case is `default`, which gets executed if none of the operations can be
// /// executed, i.e. they would block:
// ///
// /// ```
// /// use std::thread;
// /// use std::time::{Duration, Instant};
// /// use crossbeam_channel as channel;
// ///
// /// let (s, r) = channel::unbounded();
// ///
// /// thread::spawn(move || {
// ///     thread::sleep(Duration::from_secs(1));
// ///     s.send("foo");
// /// });
// ///
// /// // Don't block on the receive operation.
// /// channel::Select::new()
// ///     .recv(&r, |_| panic!())
// ///     .default(|| println!("The message is not yet available."))
// ///     .wait();
// /// ```
// ///
// /// # Execution
// ///
// /// 1. A `Select` is constructed, cases are added, and `.wait()` is called.
// /// 2. If any of the `recv` or `send` operations are ready, one of them is executed. If multiple
// ///    operations are ready, a random one is chosen.
// /// 3. If none of the `recv` and `send` operations are ready, the `default` case is executed. If
// ///    there is no `default` case, the current thread is blocked until an operation becomes ready.
// /// 4. If a `recv` operation gets executed, its callback is invoked.
// /// 5. If a `send` operation gets executed, the message is lazily evaluated and sent into the
// ///    channel. Finally, the callback is invoked.
// ///
// /// **Note**: If evaluation of the message panics, the process will be aborted because it's
// /// impossible to recover from such panics. All the other callbacks are allowed to panic, however.
// #[must_use]
// pub struct Select<'a, R> {
//     /// A list of senders and receivers participating in selection.
//     handles: SmallVec<[(&'a SelectHandle, usize, *const u8); 4]>,
//
//     /// A list of callbacks, one per handle.
//     callbacks: SmallVec<[Callback<'a, R>; 4]>,
//
//     /// Callback for the default case.
//     default: Option<Callback<'a, R>>,
// }
//
// impl<'a, R> Select<'a, R> {
//     /// Creates a new `Select`.
//     pub fn new() -> Select<'a, R> {
//         Select {
//             handles: SmallVec::new(),
//             callbacks: SmallVec::new(),
//             default: None,
//         }
//     }
//
//     /// Adds a receive case.
//     ///
//     /// The callback will get invoked if the receive operation completes.
//     #[inline]
//     pub fn recv<T, C>(mut self, r: &'a Receiver<T>, cb: C) -> Select<'a, R>
//     where
//         C: FnOnce(Option<T>) -> R + 'a,
//     {
//         let i = self.handles.len() + 1;
//         let ptr = r as *const Receiver<_> as *const u8;
//         self.handles.push((r, i, ptr));
//
//         self.callbacks.push(Callback::new(move |token| {
//             let msg = unsafe { channel::read(r, token) };
//             cb(msg)
//         }));
//
//         self
//     }
//
//     /// Adds a send case.
//     ///
//     /// If the send operation succeeds, the message will be generated and sent into the channel.
//     /// Finally, the callback gets invoked once the operation is completed.
//     ///
//     /// **Note**: If function `msg` panics, the process will be aborted because it's impossible to
//     /// recover from such panics. However, function `cb` is allowed to panic.
//     #[inline]
//     pub fn send<T, M, C>(mut self, s: &'a Sender<T>, msg: M, cb: C) -> Select<'a, R>
//     where
//         M: FnOnce() -> T + 'a,
//         C: FnOnce() -> R + 'a,
//     {
//         let i = self.handles.len() + 1;
//         let ptr = s as *const Sender<_> as *const u8;
//         self.handles.push((s, i, ptr));
//
//         self.callbacks.push(Callback::new(move |token| {
//             let _guard =
//                 utils::AbortGuard("a send case triggered a panic while evaluating its message");
//             let msg = msg();
//
//             ::std::mem::forget(_guard);
//             unsafe {
//                 channel::write(s, token, msg);
//             }
//
//             cb()
//         }));
//
//         self
//     }
//
//     /// Adds a default case.
//     ///
//     /// This case gets executed if none of the channel operations are ready.
//     ///
//     /// If called more than once, this method keeps only the last callback for the default case.
//     #[inline]
//     pub fn default<C>(mut self, cb: C) -> Select<'a, R>
//     where
//         C: FnOnce() -> R + 'a,
//     {
//         self.default = Some(Callback::new(move |_| cb()));
//         self
//     }
//
//     /// Starts selection and waits until it completes.
//     ///
//     /// The result of the executed callback function will be returned.
//     pub fn wait(mut self) -> R {
//         let (mut token, index, _) = main_loop(&mut self.handles, self.default.is_some());
//         let cb;
//
//         // Initialize `cb` with the right callback and drop all other callbacks.
//         if index == 0 {
//             self.callbacks.clear();
//             cb = self.default.take().unwrap();
//         } else {
//             cb = self.callbacks.remove(index - 1);
//             self.callbacks.clear();
//             self.default.take();
//         }
//
//         // Invoke the callback.
//         cb.call(&mut token)
//     }
// }
//
// /// Some space to keep a `FnOnce()` object on the stack.
// type Space = [usize; 2];
//
// /// A `FnOnce(&mut Token) -> R + 'a` that is stored inline if small, or otherwise boxed on the heap.
// pub struct Callback<'a, R> {
//     /// A wrapper function around the callback.
//     ///
//     /// The first argument is a pointer to `space`.
//     ///
//     /// The second argument may contain a reference to `Token`. If the argument is `Some`, the
//     /// reference is passed to the callback. If the argument is `None`, the callback is read and
//     /// dropped without invocation.
//     ///
//     /// This function may be called only once.
//     call: unsafe fn(*mut u8, token: Option<&mut Token>) -> Option<R>,
//
//     /// Some space where a function can be stored, if it fits.
//     space: Space,
//
//     /// Indicates that a `Callback` is `!Send + !Sync` and borrows `'a`.
//     _marker: PhantomData<(*mut (), &'a ())>,
// }
//
// impl<'a, R> Callback<'a, R> {
//     /// Constructs a new `Callback<'a, R>` from a `FnOnce(&mut Token) -> R + 'a`.
//     pub fn new<F>(f: F) -> Self
//     where
//         F: FnOnce(&mut Token) -> R + 'a,
//     {
//         let size = mem::size_of::<F>();
//         let align = mem::align_of::<F>();
//
//         unsafe {
//             let mut space: Space = Space::default();
//
//             let call = if size <= mem::size_of::<Space>() && align <= mem::align_of::<Space>() {
//                 unsafe fn call<'a, F, R>(raw: *mut u8, token: Option<&mut Token>) -> Option<R>
//                 where
//                     F: FnOnce(&mut Token) -> R + 'a,
//                 {
//                     // Read the function from the stack.
//                     let f: F = ptr::read(raw as *mut F);
//                     token.map(f)
//                 }
//
//                 // Write the function into the space.
//                 ptr::write(&mut space as *mut Space as *mut F, f);
//                 call::<F, R>
//             } else {
//                 unsafe fn call<'a, F, R>(raw: *mut u8, token: Option<&mut Token>) -> Option<R>
//                 where
//                     F: FnOnce(&mut Token) -> R + 'a,
//                 {
//                     // Read the pointer to the function from the stack.
//                     let b: Box<F> = ptr::read(raw as *mut Box<F>);
//                     token.map(*b)
//                 }
//
//                 // The function doesn't fit, so box it and write the pointer into the space.
//                 let b: Box<F> = Box::new(f);
//                 ptr::write(&mut space as *mut Space as *mut Box<F>, b);
//                 call::<F, R>
//             };
//
//             Callback {
//                 call,
//                 space,
//                 _marker: PhantomData,
//             }
//         }
//     }
//
//     /// Invokes the callback.
//     #[inline]
//     pub fn call(self, token: &mut Token) -> R {
//         // Disassemble `self` and forget it so that the destructor doesn't invoke `call`.
//         let Callback {
//             call, mut space, ..
//         } = self;
//         mem::forget(self);
//
//         // Invoke the callback.
//         unsafe { (call)(&mut space as *mut Space as *mut u8, Some(token)).unwrap() }
//     }
// }
//
// impl<'a, R> Drop for Callback<'a, R> {
//     fn drop(&mut self) {
//         // Call the function with `None` in order to drop the callback.
//         unsafe {
//             (self.call)(&mut self.space as *mut Space as *mut u8, None);
//         }
//     }
// }

// TODO impl Clone for Select<'a>, Debug and so on (we need some of those on SelectedCase too)
pub struct Select<'a> {
    /// A list of senders and receivers participating in selection.
    handles: SmallVec<[(&'a SelectHandle, usize, *const u8); 4]>,
}

impl<'a> Select<'a> {
    pub fn new() -> Select<'a> {
        Select {
            handles: SmallVec::new(),
        }
    }

    pub fn recv<T>(&mut self, r: &'a Receiver<T>) -> usize {
        let i = self.handles.len() + 1;
        let ptr = r as *const Receiver<_> as *const u8;
        self.handles.push((r, i, ptr));
        i - 1
    }

    pub fn send<T>(&mut self, s: &'a Sender<T>) -> usize {
        let i = self.handles.len() + 1;
        let ptr = s as *const Sender<_> as *const u8;
        self.handles.push((s, i, ptr));
        i - 1
    }

    pub fn try_select(&mut self) -> Option<SelectedCase<'_>> {
        main_loop(&mut self.handles, Timeout::Now)
            .map(|(token, index, ptr)| SelectedCase {
                token,
                index,
                ptr,
                _marker: PhantomData,
            })
    }

    pub fn select(&mut self) -> SelectedCase<'_> {
        let (token, index, ptr) = main_loop(&mut self.handles, Timeout::Never).unwrap();
        SelectedCase {
            token,
            index,
            ptr,
            _marker: PhantomData,
        }
    }

    pub fn select_timeout(&mut self, timeout: Duration) -> Option<SelectedCase<'_>> {
        let timeout = Timeout::At(Instant::now() + timeout);
        main_loop(&mut self.handles, timeout)
            .map(|(token, index, ptr)| SelectedCase {
                token,
                index,
                ptr,
                _marker: PhantomData,
            })
    }
}

#[must_use]
pub struct SelectedCase<'a> {
    token: Token,
    index: usize,
    ptr: *const u8,
    _marker: PhantomData<&'a ()>,
}

impl<'a> SelectedCase<'a> {
    pub fn index(&self) -> usize {
        self.index - 1
    }

    pub fn recv<T>(mut self, r: &'a Receiver<T>) -> Result<T, RecvError> {
        assert!(
            r as *const Receiver<T> as *const u8 == self.ptr,
            "passed a receiver that wasn't selected",
        );
        unsafe {
            channel::read(r, &mut self.token).map_err(|_| RecvError)
        }
    }

    pub fn send<T>(mut self, s: &'a Sender<T>, msg: T) -> Result<(), SendError<T>> {
        assert!(
            s as *const Sender<T> as *const u8 == self.ptr,
            "passed a sender that wasn't selected",
        );
        unsafe {
            channel::write(s, &mut self.token, msg).map_err(|m| SendError(m))
        }
    }
}

/// TODO
#[doc(hidden)]
#[macro_export]
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
        #[allow(unused_mut, unused_variables)]
        let mut sel = $crate::Select::new();
        crossbeam_channel_internal!(
            @add
            sel
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
    // Error cases...
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
    // Error cases...
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
    // Other error cases...
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

    (@add
        $sel:ident
        ()
        ()
        $labels:tt
        $cases:tt
    ) => {{
        let case: $crate::SelectedCase<'_> = {
            let case = $sel.select();
            // TODO: necessary to be able to drop sel early without NLL
            unsafe { ::std::mem::transmute(case) }
        };
        crossbeam_channel_internal! {
            @complete
            $sel
            case
            $cases
        }
    }};
    (@add
        $sel:ident
        ()
        (default() => $body:tt,)
        $labels:tt
        $cases:tt
    ) => {{
        let case: Option<$crate::SelectedCase<'_>> = {
            let case = $sel.try_select();
            // TODO: necessary to be able to drop sel early without NLL
            unsafe { ::std::mem::transmute(case) }
        };
        match case {
            None => {
                drop($sel);
                $body
            }
            Some(case) => {
                crossbeam_channel_internal! {
                    @complete
                    $sel
                    case
                    $cases
                }
            }
        }
    }};
    (@add
        $sel:ident
        ()
        (default($timeout:expr) => $body:tt,)
        $labels:tt
        $cases:tt
    ) => {{
        let case: Option<$crate::SelectedCase<'_>> = {
            let case = $sel.select_timeout($timeout);
            // TODO: necessary to be able to drop sel early without NLL
            unsafe { ::std::mem::transmute(case) }
        };
        match case {
            None => {
                drop($sel);
                $body
            }
            Some(case) => {
                crossbeam_channel_internal! {
                    @complete
                    $sel
                    case
                    $cases
                }
            }
        }
    }};
    (@add
        $sel:ident
        $input:tt
        $default:tt
        ()
        $cases:tt
    ) => {
        compile_error!("too many cases in a `select!` block")
    };
    (@add
        $sel:ident
        (recv($r:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        $default:tt
        (($i:tt $var:ident) $($labels:tt)*)
        ($($cases:tt)*)
    ) => {{
        match $r {
            ref r => {
                // TODO: this is because of NLL
                unsafe fn unbind<'a, T>(x: &T) -> &'a T {
                    ::std::mem::transmute(x)
                }
                let r: &$crate::Receiver<_> = r; // TODO
                let $var: &$crate::Receiver<_> = unsafe { unbind(r) };
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
    (@add
        $sel:ident
        (send($s:expr, $m:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        $default:tt
        (($i:tt $var:ident) $($labels:tt)*)
        ($($cases:tt)*)
    ) => {{
        match $s {
            ref s => {
                // TODO: this is because of NLL
                unsafe fn unbind<'a, T>(x: &T) -> &'a T {
                    ::std::mem::transmute(x)
                }
                let s: &$crate::Sender<_> = s; // TODO
                let $var: &$crate::Sender<_> = unsafe { unbind(s) };
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

    (@complete
        $sel:ident
        $case:ident
        ([$i:tt] recv($r:ident) -> $res:pat => $body:tt, $($tail:tt)*)
    ) => {{
        if $case.index() == $i {
            let res = $case.recv($r);
            drop($sel);
            let $res = res;
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
    (@complete
        $sel:ident
        $case:ident
        ([$i:tt] send($s:ident, $m:expr) -> $res:pat => $body:tt, $($tail:tt)*)
    ) => {{
        if $case.index() == $i {
            let res = $case.send($s, $m);
            drop($sel);
            let $res = res;
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
    (@complete
        $sel:ident
        $case:ident
        ()
    ) => {{
        unreachable!("internal error in crossbeam-channel: invalid case")
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
/// This macro allows declaring a set of channel operations and blocking until any one of them
/// becomes ready. Finally, one of the operations is executed. If multiple operations are ready at
/// the same time, a random one is chosen. It is also possible to declare a `default` case that
/// gets executed if none of the operations are initially ready.
///
/// If you need to dynamically add cases rather than define them statically inside the macro, use
/// [`Select`] instead.
///
/// [`Select`]: struct.Select.html
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
///     recv(r1, msg) => assert_eq!(msg, Ok("foo")),
///     recv(r2, msg) => assert_eq!(msg, Ok("bar")),
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
///     recv(r1, msg) => assert_eq!(msg, Ok("foo")),
///     send(s2, "bar") => assert_eq!(r2.recv(), Ok("bar")),
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
#[macro_export(local_inner_macros)]
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

    // ($($case:ident $(($($args:tt)*))* $(-> $res:pat)* => $body:expr $(,)*)*) => {
    //     crossbeam_channel_internal!(
    //         $($case $(($($args)*))* $(-> $res)* => { $body },)*
    //     )
    // };
    //
    ($($tokens:tt)*) => {
        crossbeam_channel_internal!(
            $($tokens)*
        )
    };
}
