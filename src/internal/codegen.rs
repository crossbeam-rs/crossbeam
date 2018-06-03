/// Code generator for the `select!` macro.

use std::option;
use std::time::Instant;

use internal::channel::{Receiver, Sender};
use internal::context;
use internal::select::{CaseId, Select, Token};
use internal::utils;

pub fn mainloop<'a, S>(
    cases: &mut [(&'a S, usize, usize)],
    default_index: usize,
) -> (Token, usize, usize)
where
    S: Select + ?Sized + 'a,
{
    let mut token: Token = Default::default();

    if cases.len() >= 2 {
        utils::shuffle(cases);
    }

    if cases.is_empty() {
        if default_index != !0 {
            return (token, default_index, 0);
        } else {
            utils::sleep_forever();
        }
    }

    loop {
        for &(sel, i, addr) in cases.iter() {
            if sel.try(&mut token) {
                return (token, i, addr);
            }
        }

        if default_index != !0 {
            return (token, default_index, 0);
        }

        for &(sel, i, addr) in cases.iter() {
            if sel.retry(&mut token) {
                return (token, i, addr);
            }
        }

        context::current_reset();

        for case in cases.iter() {
            let case_id = CaseId::new(case as *const _ as usize);
            let &(sel, _, _) = case;

            if !sel.register(&mut token, case_id) {
                context::current_try_abort();
                break;
            }

            if context::current_selected() != CaseId::none() {
                break;
            }
        }

        let mut deadline: Option<Instant> = None;
        for &(sel, _, _) in cases.iter() {
            if let Some(x) = sel.deadline() {
                deadline = deadline.map(|y| x.min(y)).or(Some(x));
            }
        }

        context::current_wait_until(deadline);
        let s = context::current_selected();

        for case in cases.iter() {
            let case_id = CaseId::new(case as *const _ as usize);
            let &(sel, _, _) = case;
            sel.unregister(case_id);
        }

        if s != CaseId::abort() {
            for case in cases.iter() {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, i, addr) = case;

                if case_id == s {
                    if sel.accept(&mut token) {
                        return (token, i, addr);
                    }
                }
            }
        }

        if cases.len() >= 2 {
            utils::shuffle(cases);
        }
    }
}

pub unsafe fn bind_address<'a, T: 'a, I>(_: &I, addr: usize) -> &'a T
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

// TODO: Try supporting `IntoIterator<Item = &'a &'a Receiver<T>>` once we get specialization.
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

        let default_index: usize; // TODO: turn into has_default
        __crossbeam_channel_codegen!(@default default_index $default);

        #[allow(unused_mut)]
        #[allow(unused_variables)]
        let (mut token, index, selected) = $crate::internal::codegen::mainloop(
            &mut $cases,
            default_index,
        );

        __crossbeam_channel_codegen!(@finish token index selected $recv $send $default)
    }};

    (@container
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::Receiver<_>, usize, usize); 4]
        >::new();
        while let Some(r) = $var.next() {
            let addr = r as *const $crate::Receiver<_> as usize;
            c.push((r, $i, addr));
        }
        c
    }};
    (@container
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt,)
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::Sender<_>, usize, usize); 4]
        >::new();
        while let Some(s) = $var.next() {
            let addr = s as *const $crate::Sender<_> as usize;
            c.push((s, $i, addr));
        }
        c
    }};
    (@container
        $recv:tt
        $send:tt
    ) => {{
        let mut c = $crate::internal::smallvec::SmallVec::<
            [(&$crate::internal::select::Select, usize, usize); 4]
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
            let addr = r as *const $crate::Receiver<_> as usize;
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
            let addr = s as *const $crate::Sender<_> as usize;
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

    (@default
        $default_index:ident
        ()
    ) => {
        $default_index = !0;
    };
    (@default
        $default_index:ident
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        $default_index = $i;
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
