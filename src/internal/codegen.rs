/// Code generator for the `select!` macro.

use std::option;
use std::ptr;
use std::time::Instant;

use internal::channel::{Receiver, Sender};
use internal::context;
use internal::select::{CaseId, Select, Token};
use internal::utils;

// TODO: rename CaseId to OperationId and cases to operation

/// Runs until one of the operations is fired, potentially blocking the current thread.
///
/// Receive operations will have to be followed up by `read`, and send operations by `write`.
pub fn mainloop<'a, S>(
    cases: &mut [(&'a S, usize, *const u8)],
    has_default: bool,
) -> (Token, usize, *const u8)
where
    S: Select + ?Sized + 'a,
{
    // Create a token, which serves as a temporary variable that gets initialized in this function
    // and is later used by a call to `read` or `write` that completes the selected operation.
    let mut token = Token::default();

    // Shuffle the cases for fairness.
    if cases.len() >= 2 {
        utils::shuffle(cases);
    }

    if cases.is_empty() {
        if has_default {
            // If there is only the default case, return.
            return (token, 0, ptr::null());
        } else {
            // If there are no cases at all, block forever.
            utils::sleep_forever();
        }
    }

    loop {
        // Try firing the cases without blocking.
        for &(select, i, addr) in cases.iter() {
            if select.try(&mut token) {
                return (token, i, addr);
            }
        }

        // If there's a default case, select it.
        if has_default {
            return (token, 0, ptr::null());
        }

        // Before blocking, try firing the cases one more time. Retries are permitted to take a
        // little bit more time than the initial tries, but they still mustn't block.
        for &(select, i, addr) in cases.iter() {
            if select.retry(&mut token) {
                return (token, i, addr);
            }
        }

        // Prepare for blocking.
        context::current_reset();
        let mut sel = CaseId::Waiting;
        let mut registered_count = 0;

        // Register all cases.
        for case in cases.iter_mut() {
            let &mut (select, _, _) = case;
            registered_count += 1;

            // If registration returns `false`, that means the operation has just become ready.
            if !select.register(&mut token, CaseId::hook(case)) {
                // Abort blocking and then again.
                sel = context::current_try_abort();
                break;
            }

            // If another thread has already selected one of the cases, stop registration.
            sel = context::current_selected();
            if sel != CaseId::Waiting {
                break;
            }
        }

        if sel == CaseId::Waiting {
            // Check with each case how long we're allowed to block.
            let mut deadline: Option<Instant> = None;
            for &(select, _, _) in cases.iter() {
                if let Some(x) = select.deadline() {
                    deadline = deadline.map(|y| x.min(y)).or(Some(x));
                }
            }

            // Block the current thread.
            sel = context::current_wait_until(deadline);
        }

        // Unregister all registered cases.
        for case in cases.iter_mut().take(registered_count) {
            let &mut (select, _, _) = case;
            select.unregister(CaseId::hook(case));
        }

        match sel {
            CaseId::Waiting => unreachable!(),
            CaseId::Aborted => {},
            CaseId::Closed | CaseId::Case(_) => {
                // Find the selected case.
                for case in cases.iter_mut() {
                    let &mut (select, i, addr) = case;

                    // Is this the selected case?
                    if sel == CaseId::hook(case) {
                        // Try firing this case.
                        if select.accept(&mut token) {
                            return (token, i, addr);
                        }
                    }
                }

                // Before the next round, reshuffle the cases for fairness.
                if cases.len() >= 2 {
                    utils::shuffle(cases);
                }
            },
        }
    }
}

pub unsafe fn bind_address<'a, T: 'a, I>(_: &I, addr: *const u8) -> &'a T
where
    I: Iterator<Item = &'a T>,
{
    &*(addr as *const T)
}

pub trait RecvArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Receiver<T>>;

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

// TODO: Try supporting `IntoIterator<Item = &'a &'b Receiver<T>>` once we get specialization.
impl<'a, T: 'a, I: IntoIterator<Item = &'a Receiver<T>> + Clone> RecvArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;

    fn __as_recv_argument(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

pub trait SendArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Sender<T>>;

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

// TODO: Try supporting `IntoIterator<Item = &'a &'b Sender<T>>` once we get specialization.
impl<'a, T: 'a, I: IntoIterator<Item = &'a Sender<T>> + Clone> SendArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;

    fn __as_send_argument(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

/// TODO
#[macro_export]
#[doc(hidden)]
macro_rules! __crossbeam_channel_codegen {
    (@declare
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        {
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
        }
    };
    (@declare
        (($i:tt $var:ident) send($ss:expr, $m:pat, $s:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        {
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
        }
    };
    (@declare
        ()
        $recv:tt
        $send:tt
        $default:tt
    ) => {{
        let mut cases = __crossbeam_channel_codegen!(@container $recv $send);
        __crossbeam_channel_codegen!(@fastpath $recv $send $default cases)
    }};

    (@fastpath
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
        ()
        $cases:ident
    ) => {{
        if $cases.len() == 1 {
            let $r = $cases[0].0;
            let $m = $cases[0].0.recv();
            $body
        } else {
            __crossbeam_channel_codegen!(
                @mainloop
                (($i $var) recv($rs, $m, $r) => $body,)
                ()
                ()
                $cases
            )
        }
    }};
    (@fastpath
        (($recv_i:tt $recv_var:ident) recv($rs:expr, $m:pat, $r:pat) => $recv_body:tt,)
        ()
        (($default_i:tt $default_var:ident) default() => $default_body:tt,)
        $cases:ident
    ) => {{
        if $cases.len() == 1 {
            let r = $cases[0].0;
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
                @mainloop
                (($recv_i $recv_var) recv($rs, $m, $r) => $recv_body,)
                ()
                (($default_i $default_var) default() => $default_body,)
                $cases
            )
        }
    }};
    (@fastpath
        $recv:tt
        $send:tt
        $default:tt
        $cases:ident
    ) => {{
        __crossbeam_channel_codegen!(@mainloop $recv $send $default $cases)
    }};
    // TODO: Optimize `select! { send(s, msg) => {} }`.
    // TODO: Optimize `select! { send(s, msg) => {} default => {} }`.

    (@mainloop
        $recv:tt
        $send:tt
        $default:tt
        $cases:ident
    ) => {{
        // TODO: set up a guard that aborts if anything panics before actually finishing

        let has_default = __crossbeam_channel_codegen!(@has_default $default);

        #[allow(unused_mut)]
        #[allow(unused_variables)]
        let (mut token, index, selected) = $crate::internal::codegen::mainloop(
            &mut $cases,
            has_default,
        );

        __crossbeam_channel_codegen!(@finish token index selected $recv $send $default)
    }};

    (@container
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::Receiver<_>, usize, *const u8); 4]
        >::new();
        while let Some(r) = $var.next() {
            let addr = r as *const $crate::Receiver<_> as *const u8;
            c.push((r, $i, addr));
        }
        c
    }};
    (@container
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt,)
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::Sender<_>, usize, *const u8); 4]
        >::new();
        while let Some(s) = $var.next() {
            let addr = s as *const $crate::Sender<_> as *const u8;
            c.push((s, $i, addr));
        }
        c
    }};
    (@container
        $recv:tt
        $send:tt
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::internal::select::Select, usize, *const u8); 4]
        >::new();
        __crossbeam_channel_codegen!(@push c $recv $send);
        c
    }};

    (@push
        $cases:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        while let Some(r) = $var.next() {
            let addr = r as *const $crate::Receiver<_> as *const u8;
            $cases.push((r, $i, addr));
        }
        __crossbeam_channel_codegen!(@push $cases ($($tail)*) $send);
    };
    (@push
        $cases:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        while let Some(s) = $var.next() {
            let addr = s as *const $crate::Sender<_> as *const u8;
            $cases.push((s, $i, addr));
        }
        __crossbeam_channel_codegen!(@push $cases () ($($tail)*));
    };
    (@push
        $cases:ident
        ()
        ()
    ) => {
    };

    (@has_default
        ()
    ) => {
        false
    };
    (@has_default
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        true
    };

    (@finish
        $token:ident
        $index:ident
        $selected:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
        $default:tt
    ) => {
        if $index == $i {
            let ($m, $r) = unsafe {
                let r = $crate::internal::codegen::bind_address(&$var, $selected);
                let msg = $crate::internal::channel::read(r, &mut $token);
                (msg, r)
            };
            $body
        } else {
            __crossbeam_channel_codegen!(
                @finish
                $token
                $index
                $selected
                ($($tail)*)
                $send
                $default
            )
        }
    };
    (@finish
        $token:ident
        $index:ident
        $selected:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
        $default:tt
    ) => {
        if $index == $i {
            let $s = {
                // We have to prefix variables with an underscore to get rid of warnings in
                // case `$m` is of type `!`.
                let _s = unsafe { $crate::internal::codegen::bind_address(&$var, $selected) };
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
                @finish
                $token
                $index
                $selected
                ()
                ($($tail)*)
                $default
            )
        }
    };
    (@finish
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
                @finish
                $token
                $index
                $selected
                ()
                ()
                ()
            )
        }
    };
    (@finish
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
