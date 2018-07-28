/// Code generator for the `select!` macro.

use std::option;
use std::ptr;
use std::time::Instant;

use internal::channel::{Receiver, Sender};
use internal::context;
use internal::select::{Operation, Select, SelectHandle, Token};
use internal::utils;

use internal::smallvec::SmallVec;

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
        'start: loop {
            // Snapshot the channel state of all operations.
            let mut states = SmallVec::<[usize; 4]>::new();
            for &(handle, _, _) in handles.iter() {
                states.push(handle.state());
            }

            for &(handle, i, ptr) in handles.iter() {
                if handle.try(&mut token) {
                    return (token, i, ptr);
                }
            }

            // If any of the states has just changed, jump to the beginning of the main loop.
            for (&(handle, _, _), &state) in handles.iter().zip(states.iter()) {
                if handle.state() != state {
                    continue 'start;
                }
            }

            // Select the `default` case.
            return (token, 0, ptr::null());
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
            // Select the `default` case.
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
        context::current_reset();
        let mut sel = Select::Waiting;
        let mut registered_count = 0;

        // Register all operations.
        for (handle, _, _) in handles.iter_mut() {
            registered_count += 1;

            // If registration returns `false`, that means the operation has just become ready.
            if !handle.register(&mut token, Operation::hook(handle)) {
                // Try aborting select.
                sel = match context::current_try_select(Select::Aborted) {
                    Ok(()) => Select::Aborted,
                    Err(s) => s,
                };
                break;
            }

            // If another thread has already selected one of the operations, stop registration.
            sel = context::current_selected();
            if sel != Select::Waiting {
                break;
            }
        }

        if sel == Select::Waiting {
            // Check with each operation for how long we're allowed to block, and compute the
            // earliest deadline.
            let mut deadline: Option<Instant> = None;
            for &(handle, _, _) in handles.iter() {
                if let Some(x) = handle.deadline() {
                    deadline = deadline.map(|y| x.min(y)).or(Some(x));
                }
            }

            // Block the current thread.
            sel = context::current_wait_until(deadline);
        }

        // Unregister all registered operations.
        for (handle, _, _) in handles.iter_mut().take(registered_count) {
            handle.unregister(Operation::hook(handle));
        }

        match sel {
            Select::Waiting => unreachable!(),
            Select::Aborted => {},
            Select::Closed | Select::Operation(_) => {
                // Find the selected operation.
                for (handle, i, ptr) in handles.iter_mut() {
                    // Is this the selected operation?
                    if sel == Select::Operation(Operation::hook(handle)) {
                        // Try firing this operation.
                        if handle.accept(&mut token) {
                            return (token, *i, *ptr);
                        }
                    }
                }

                // Before the next round, reshuffle the operations for fairness.
                if handles.len() >= 2 {
                    utils::shuffle(handles);
                }
            },
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

/// Generates code that performs select over a set of cases.
///
/// The input to this macro is the output from the parser macro.
#[macro_export]
#[doc(hidden)]
macro_rules! __crossbeam_channel_codegen {
    // Declare the iterator variable for a `recv` case.
    (@declare
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        match {
            #[allow(unused_imports)]
            use $crate::internal::codegen::RecvArgument;
            &mut (&&$rs).__as_recv_argument()
        } {
            $var => {
                __crossbeam_channel_codegen!(
                    @declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    }};
    // Declare the iterator variable for a `send` case.
    (@declare
        (($i:tt $var:ident) send($ss:expr, $m:pat, $s:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        match {
            #[allow(unused_imports)]
            use $crate::internal::codegen::SendArgument;
            &mut (&&$ss).__as_send_argument()
        } {
            $var => {
                __crossbeam_channel_codegen!(
                    @declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    }};
    // All iterator variables have been declared.
    (@declare
        ()
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        #[cfg_attr(feature = "cargo-clippy", allow(clippy))]
        let mut handles = __crossbeam_channel_codegen!(@container $recv $send);
        __crossbeam_channel_codegen!(@fast_path $recv $send $default handles)
    }};

    // Attempt to optimize the whole `select!` into a single call to `recv`.
    (@fast_path
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
            __crossbeam_channel_codegen!(
                @main_loop
                (($i $var) recv($rs, $m, $r) => $body,)
                ()
                ()
                $handles
            )
        }
    }};
    // Attempt to optimize the whole `select!` into a single call to `recv_nonblocking`.
    (@fast_path
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
            __crossbeam_channel_codegen!(
                @main_loop
                (($recv_i $recv_var) recv($rs, $m, $r) => $recv_body,)
                ()
                (($default_i $default_var) default() => $default_body,)
                $handles
            )
        }
    }};
    // Move on to the main select loop.
    (@fast_path
        $recv:tt
        $send:tt
        $default:tt
        $handles:ident
    ) => {{
        __crossbeam_channel_codegen!(
            @main_loop
            $recv
            $send
            $default
            $handles
        )
    }};
    // TODO: Optimize `select! { send(s, msg) => {} }`.
    // TODO: Optimize `select! { send(s, msg) => {} default => {} }`.

    // The main select loop.
    (@main_loop
        $recv:tt
        $send:tt
        $default:tt
        $handles:ident
    ) => {{
        // Check if there's a `default` case.
        let has_default = __crossbeam_channel_codegen!(@has_default $default);

        // Run the main loop.
        #[allow(unused_mut)]
        #[allow(unused_variables)]
        let (mut token, index, selected) = $crate::internal::codegen::main_loop(
            &mut $handles,
            has_default,
        );

        // Pass the result of the main loop to the final step.
        __crossbeam_channel_codegen!(
            @finalize
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
    (@container
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
    (@container
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
    (@container
        $recv:tt
        $send:tt
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::internal::select::SelectHandle, usize, *const u8); 4]
        >::new();
        __crossbeam_channel_codegen!(@push c $recv $send);
        c
    }};

    // Push a `recv` operation into the `handles` vector.
    (@push
        $handles:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        while let Some(r) = $var.next() {
            let ptr = r as *const $crate::Receiver<_> as *const u8;
            $handles.push((r, $i, ptr));
        }
        __crossbeam_channel_codegen!(@push $handles ($($tail)*) $send);
    };
    // Push a `send` operation into the `handles` vector.
    (@push
        $handles:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        while let Some(s) = $var.next() {
            let ptr = s as *const $crate::Sender<_> as *const u8;
            $handles.push((s, $i, ptr));
        }
        __crossbeam_channel_codegen!(@push $handles () ($($tail)*));
    };
    // There are no more operations to push.
    (@push
        $handles:ident
        ()
        ()
    ) => {
    };

    // Evaluate to `false` if there is no `default` case.
    (@has_default
        ()
    ) => {
        false
    };
    // Evaluate to `true` if there is a `default` case.
    (@has_default
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        true
    };

    // Finalize a receive operation.
    (@finalize
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
                let r = $crate::internal::codegen::deref_from_iterator(
                    $selected as *const $crate::Receiver<_>,
                    &$var,
                );
                let msg = $crate::internal::channel::read(r, &mut $token);
                (msg, r)
            };

            drop($handles);
            $body
        } else {
            __crossbeam_channel_codegen!(
                @finalize
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
    (@finalize
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
                    $crate::internal::codegen::deref_from_iterator(
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
            __crossbeam_channel_codegen!(
                @finalize
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
    (@finalize
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
            __crossbeam_channel_codegen!(
                @finalize
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
    (@finalize
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

    // The entry point.
    (($($recv:tt)*) ($($send:tt)*) $default:tt) => {
        __crossbeam_channel_codegen!(
            @declare
            ($($recv)* $($send)*)
            ($($recv)*)
            ($($send)*)
            $default
        )
    }
}
