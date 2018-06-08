/// Code generator for the `select!` macro.

use std::option;
use std::ptr;
use std::time::Instant;

use internal::channel::{Receiver, Sender};
use internal::context;
use internal::select::{Select, SelectHandle, Token};
use internal::utils;

/// Runs until one of the operations is fired, potentially blocking the current thread.
///
/// Receive operations will have to be followed up by `read`, and send operations by `write`.
pub fn main_loop<'a, S>(
    operations: &mut [(&'a S, usize, *const u8)],
    has_default: bool,
) -> (Token, usize, *const u8)
where
    S: SelectHandle + ?Sized + 'a,
{
    // Create a token, which serves as a temporary variable that gets initialized in this function
    // and is later used by a call to `read` or `write` that completes the selected operation.
    let mut token = Token::default();

    // Shuffle the operations for fairness.
    if operations.len() >= 2 {
        utils::shuffle(operations);
    }

    if operations.is_empty() {
        if has_default {
            // If there is only the `default` case, return.
            return (token, 0, ptr::null());
        } else {
            // If there are no operations at all, block forever.
            utils::sleep_forever();
        }
    }

    loop {
        // Try firing the operations without blocking.
        for &(select, i, ptr) in operations.iter() {
            if select.try(&mut token) {
                return (token, i, ptr);
            }
        }

        // If there's a `default` case, select it.
        if has_default {
            return (token, 0, ptr::null());
        }

        // Before blocking, try firing the operations one more time. Retries are permitted to take
        // a little bit more time than the initial tries, but they still mustn't block.
        for &(select, i, ptr) in operations.iter() {
            if select.retry(&mut token) {
                return (token, i, ptr);
            }
        }

        // Prepare for blocking.
        context::current_reset();
        let mut sel = Select::Waiting;
        let mut registered_count = 0;

        // Register all operations.
        for operation in operations.iter_mut() {
            let &mut (select, _, _) = operation;
            registered_count += 1;

            // If registration returns `false`, that means the operation has just become ready.
            if !select.register(&mut token, Select::hook(operation)) {
                // Abort blocking and then again.
                sel = context::current_try_abort();
                break;
            }

            // If another thread has already selected one of the operations, stop registration.
            sel = context::current_selected();
            if sel != Select::Waiting {
                break;
            }
        }

        if sel == Select::Waiting {
            // Check with each operation how long we're allowed to block.
            let mut deadline: Option<Instant> = None;
            for &(select, _, _) in operations.iter() {
                if let Some(x) = select.deadline() {
                    deadline = deadline.map(|y| x.min(y)).or(Some(x));
                }
            }

            // Block the current thread.
            sel = context::current_wait_until(deadline);
        }

        // Unregister all registered operations.
        for operation in operations.iter_mut().take(registered_count) {
            let &mut (select, _, _) = operation;
            select.unregister(Select::hook(operation));
        }

        match sel {
            Select::Waiting => unreachable!(),
            Select::Aborted => {},
            Select::Closed | Select::Selected(_) => {
                // Find the selected operation.
                for operation in operations.iter_mut() {
                    let &mut (select, i, ptr) = operation;

                    // Is this the selected operation?
                    if sel == Select::hook(operation) {
                        // Try firing this operation.
                        if select.accept(&mut token) {
                            return (token, i, ptr);
                        }
                    }
                }

                // Before the next round, reshuffle the operations for fairness.
                if operations.len() >= 2 {
                    utils::shuffle(operations);
                }
            },
        }
    }
}

/// Dereference the pointer and bind it to the lifetime in the iterator.
///
/// The returned reference will appear as if it was previously produced by the iterator.
pub unsafe fn deref_from_iterator<'a, T: 'a, I>(ptr: *const u8, _: &I) -> &'a T
where
    I: Iterator<Item = &'a T>,
{
    &*(ptr as *const T)
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

/// Generates code that performs selection over a set of cases.
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
        let mut operations = __crossbeam_channel_codegen!(@container $recv $send);
        __crossbeam_channel_codegen!(@fast_path $recv $send $default operations)
    }};

    // Attempt to optimize the whole `select!` into a single call to `recv`.
    (@fast_path
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
        ()
        $operations:ident
    ) => {{
        if $operations.len() == 1 {
            let $r = $operations[0].0;
            let $m = $operations[0].0.recv();
            $body
        } else {
            __crossbeam_channel_codegen!(
                @main_loop
                (($i $var) recv($rs, $m, $r) => $body,)
                ()
                ()
                $operations
            )
        }
    }};
    // Attempt to optimize the whole `select!` into a single call to `recv_nonblocking`.
    (@fast_path
        (($recv_i:tt $recv_var:ident) recv($rs:expr, $m:pat, $r:pat) => $recv_body:tt,)
        ()
        (($default_i:tt $default_var:ident) default() => $default_body:tt,)
        $operations:ident
    ) => {{
        if $operations.len() == 1 {
            let r = $operations[0].0;
            let msg;

            match $crate::internal::channel::recv_nonblocking(r) {
                $crate::internal::channel::RecvNonblocking::Message(m) => {
                    msg = Some(m);
                    let $m = msg;
                    let $r = r;
                    $recv_body
                }
                $crate::internal::channel::RecvNonblocking::Closed => {
                    msg = None;
                    let $m = msg;
                    let $r = r;
                    $recv_body
                }
                $crate::internal::channel::RecvNonblocking::Empty => {
                    $default_body
                }
            }
        } else {
            __crossbeam_channel_codegen!(
                @main_loop
                (($recv_i $recv_var) recv($rs, $m, $r) => $recv_body,)
                ()
                (($default_i $default_var) default() => $default_body,)
                $operations
            )
        }
    }};
    // Move on to the main selection loop.
    (@fast_path
        $recv:tt
        $send:tt
        $default:tt
        $operations:ident
    ) => {{
        __crossbeam_channel_codegen!(@main_loop $recv $send $default $operations)
    }};
    // TODO: Optimize `select! { send(s, msg) => {} }`.
    // TODO: Optimize `select! { send(s, msg) => {} default => {} }`.

    // The main selection loop.
    (@main_loop
        $recv:tt
        $send:tt
        $default:tt
        $operations:ident
    ) => {{
        // TODO: set up a guard that aborts if anything panics before actually finishing

        // Check if there's a `default` case.
        let has_default = __crossbeam_channel_codegen!(@has_default $default);

        // Run the main loop.
        #[allow(unused_mut)]
        #[allow(unused_variables)]
        let (mut token, index, selected) = $crate::internal::codegen::main_loop(
            &mut $operations,
            has_default,
        );

        // Pass the result of the main loop to the final step.
        __crossbeam_channel_codegen!(@finalize token index selected $recv $send $default)
    }};

    // Initialize the `operations` vector if there's only a single `recv` case.
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
    // Initialize the `operations` vector if there's only a single `send` case.
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
    // Initialize the `operations` vector generically.
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

    // Push a `recv` operation into the `operations` vector.
    (@push
        $operations:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        while let Some(r) = $var.next() {
            let ptr = r as *const $crate::Receiver<_> as *const u8;
            $operations.push((r, $i, ptr));
        }
        __crossbeam_channel_codegen!(@push $operations ($($tail)*) $send);
    };
    // Push a `send` operation into the `operations` vector.
    (@push
        $operations:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        while let Some(s) = $var.next() {
            let ptr = s as *const $crate::Sender<_> as *const u8;
            $operations.push((s, $i, ptr));
        }
        __crossbeam_channel_codegen!(@push $operations () ($($tail)*));
    };
    // There are no more operations to push.
    (@push
        $operations:ident
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
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
        $default:tt
    ) => {
        if $index == $i {
            let ($m, $r) = unsafe {
                let r = $crate::internal::codegen::deref_from_iterator($selected, &$var);
                let msg = $crate::internal::channel::read(r, &mut $token);
                (msg, r)
            };
            $body
        } else {
            __crossbeam_channel_codegen!(
                @finalize
                $token
                $index
                $selected
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
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
        $default:tt
    ) => {
        if $index == $i {
            let $s = {
                // We have to prefix variables with an underscore to get rid of warnings when
                // evaluation of `$m` doesn't finish.
                let _s = unsafe {
                    $crate::internal::codegen::deref_from_iterator($selected, &$var)
                };
                let _guard = $crate::internal::utils::AbortGuard(
                    "a send case triggered a panic while evaluating its message"
                );
                let _msg = $m;

                #[allow(unreachable_code)]
                {
                    ::std::mem::forget(_guard);
                    unsafe { $crate::internal::channel::write(_s, &mut $token, _msg); }
                    _s
                }
            };
            $body
        } else {
            __crossbeam_channel_codegen!(
                @finalize
                $token
                $index
                $selected
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
        ()
        ()
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        if $index == $i {
            $body
        } else {
            __crossbeam_channel_codegen!(
                @finalize
                $token
                $index
                $selected
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
