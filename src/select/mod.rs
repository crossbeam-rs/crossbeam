pub mod handle;

use smallvec::SmallVec;
use channel::{Token, PreparedSender, Receiver, Sender};
use utils::Backoff;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CaseId {
    id: usize,
}

impl CaseId {
    #[inline]
    pub fn none() -> Self {
        CaseId { id: 0 }
    }

    #[inline]
    pub fn abort() -> Self {
        CaseId { id: 1 }
    }

    #[inline]
    pub fn new(id: usize) -> Self {
        CaseId { id }
    }
}

impl From<usize> for CaseId {
    #[inline]
    fn from(id: usize) -> Self {
        CaseId { id }
    }
}

impl Into<usize> for CaseId {
    #[inline]
    fn into(self) -> usize {
        self.id
    }
}

#[macro_export]
macro_rules! select {
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        select_internal!(@parse_list () ($($case $(($($args)*))* => $body,)*))
    };
    ($($tokens:tt)*) => {
        select_internal!(@parse_list () ($($tokens)*))
    };
}

pub trait Sel {
    type Token;
    fn try(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool;
    fn promise(&self, case_id: CaseId);
    fn is_blocked(&self) -> bool;
    fn revoke(&self, case_id: CaseId);
    fn fulfill(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool;
    fn finish(&self, token: &mut Self::Token);
    fn fail(&self, token: &mut Self::Token);
}

impl<'a, T: Sel> Sel for &'a T {
    type Token = <T as Sel>::Token;
    fn try(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool {
        (**self).try(token, backoff)
    }
    fn promise(&self, case_id: CaseId) {
        (**self).promise(case_id);
    }
    fn is_blocked(&self) -> bool {
        (**self).is_blocked()
    }
    fn revoke(&self, case_id: CaseId) {
        (**self).revoke(case_id);
    }
    fn fulfill(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool {
        (**self).fulfill(token, backoff)
    }
    fn finish(&self, token: &mut Self::Token) {
        (**self).finish(token)
    }
    fn fail(&self, token: &mut Self::Token) {
        (**self).fail(token);
    }
}

pub trait RecvArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Receiver<T>>;
    fn to_receivers(&'a self) -> Self::Iter;
}

impl<'a, T> RecvArgument<'a, T> for &'a Receiver<T> {
    type Iter = ::std::option::IntoIter<&'a Receiver<T>>;
    fn to_receivers(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Receiver<T>> + Clone> RecvArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;
    fn to_receivers(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

pub trait SendArgument<'a, S: 'a> {
    type Iter: Iterator<Item = &'a S>;
    fn to_senders(&'a self) -> Self::Iter;
}

impl<'a, T> SendArgument<'a, Sender<T>> for &'a Sender<T> {
    type Iter = ::std::option::IntoIter<&'a Sender<T>>;
    fn to_senders(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T> SendArgument<'a, PreparedSender<'a, T>> for &'a PreparedSender<'a, T> {
    type Iter = ::std::option::IntoIter<&'a PreparedSender<'a, T>>;
    fn to_senders(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Sender<T>> + Clone> SendArgument<'a, Sender<T>> for I {
    type Iter = <I as IntoIterator>::IntoIter;
    fn to_senders(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! select_internal {
    // Success! The list is empty.
    (@parse_list ($($head:tt)*) ()) => {
        select_internal!(
            @parse_case
            ()
            ()
            ()
            ($($head)*)
            (
                (0usize case0) (1usize case1) (2usize case2) (3usize case3) (4usize case4)
                (5usize case5) (6usize case6) (7usize case7) (8usize case8) (9usize case9)
                (10usize case10) (11usize case11) (12usize case12) (13usize case13)
                (14usize case14) (15usize case15) (16usize case16) (17usize case17)
                (20usize case18) (19usize case19) (20usize case20) (21usize case21)
                (22usize case22) (23usize case23) (24usize case24) (25usize case25)
                (26usize case26) (27usize case27) (28usize case28) (29usize case29)
                (30usize case30) (31usize case31)
            )
        )
    };
    // If necessary, insert an empty argument list after `default`.
    (@parse_list
        ($($head:tt)*)
        (default => $($tail:tt)*)
    ) => {
        select_internal!(@parse_list ($($head)*) (default() => $($tail)*))
    };
    // The first case is separated by a comma.
    (@parse_list
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr, $($tail:tt)*)
    ) => {
        select_internal!(
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
        select_internal!(
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
        select_internal!(
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
        select_internal!(
            @parse_list
            ($($head)* $case $args => { $body },)
            ()
        )
    };
    // Diagnose and print an error.
    (@parse_list ($($head:tt)*) ($($tail:tt)*)) => {
        select_internal!(@parse_list_error1 $($tail)*)
    };
    // Stage 1: check the case type.
    (@parse_list_error1 recv $($tail:tt)*) => {
        select_internal!(@parse_list_error2 recv $($tail)*)
    };
    (@parse_list_error1 send $($tail:tt)*) => {
        select_internal!(@parse_list_error2 send $($tail)*)
    };
    (@parse_list_error1 default $($tail:tt)*) => {
        select_internal!(@parse_list_error2 default $($tail)*)
    };
    (@parse_list_error1 $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected one of `recv`, `send`, or `default`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@parse_list_error1 $($tail:tt)*) => {
        select_internal!(@parse_list_error2 $($tail)*);
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
        select_internal!(@parse_list_error3 $($tail)*)
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
        select_internal!(@parse_list_error4 $($tail)*)
    };
    // Stage 4: fail with a generic error message.
    (@parse_list_error4 $($tail:tt)*) => {
        compile_error!("invalid syntax")
    };

    // Success! All cases were consumed.
    (@parse_case
        ($($recv:tt)*)
        ($($send:tt)*)
        $default:tt
        ()
        $labels:tt
    ) => {
        select_internal!(
            @declare
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
        compile_error!("too many cases in a `select_internal!` block")
    };
    // Check the format of a `recv` case...
    (@parse_case
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select_internal!(
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
        select_internal!(
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
        (recv($r:expr, $m:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select_internal!(
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
        select_internal!(
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
            "invalid arguments in `recv(",
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
        select_internal!(
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
        select_internal!(
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
        select_internal!(
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
        select_internal!(
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
            "invalid arguments in `send(",
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
        ($label:tt $($labels:tt)*)
    ) => {
        select_internal!(
            @parse_case
            $recv
            $send
            ($label default() => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@parse_case
        $recv:tt
        $send:tt
        ()
        (default($t:expr) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select_internal!(
            @parse_case
            $recv
            $send
            ($label default($t) => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Allow trailing comma...
    (@parse_case
        $recv:tt
        $send:tt
        ()
        (default($t:expr,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select_internal!(
            @parse_case
            $recv
            $send
            ($label default($t) => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Valid, but duplicate cases...
    (@parse_case
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default() => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select_internal!` block")
    };
    (@parse_case
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default($t:expr) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select_internal!` block")
    };
    (@parse_case
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default($t:expr,) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select_internal!` block")
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
            "invalid arguments in `default(",
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

    (@declare
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        {
            use $crate::select::RecvArgument;
            use $crate::channel::Receiver;
            match &mut (&$rs).to_receivers() {
                $var => {
                    select_internal!(
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
            use $crate::select::SendArgument;
            use $crate::channel::Sender;
            match &mut (&$ss).to_senders() {
                $var => {
                    select_internal!(
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
    ) => {
        select_internal!(@mainloop $recv $send $default)
    };

    (@mainloop $recv:tt $send:tt $default:tt) => {{
        use std::time::Instant;

        // These cause warnings:
        use $crate::select::Sel;
        use $crate::select::SendArgument;

        use $crate::channel::Receiver;
        use $crate::channel::Sender;
        use $crate::channel::Token;
        use $crate::select::CaseId;
        use $crate::select::handle;
        use $crate::utils::{Backoff, shuffle};

        let deadline: Option<Instant>;
        let default_index: usize;
        select_internal!(@default deadline default_index $default);

        let mut cases;
        select_internal!(@container cases $recv $send);

        let mut token: Token = unsafe { ::std::mem::zeroed() };
        let mut index: usize = !0;
        let mut selected: usize = 0;

        // TODO: if cases.len() == 0, just sleep or whatever
        // TODO: if cases.len() == 1, then call optimized try, or else call send_timeout or recv_timeout

        // TODO: remove `type Token` from Sel - that would allow us to push a flavor impl as &Sel
        // - also: it would allow us to specialize selects per flavor in recv/send/try_recv

        shuffle(&mut cases);
        loop {
            // TODO: Tune backoff for zero flavor performance (too much yielding is bad)
            let backoff = &mut Backoff::new();
            loop {
                for &(sel, i, addr) in &cases {
                    if sel.try(&mut token, backoff) {
                        index = i;
                        selected = addr;
                        break;
                    }
                }

                if index != !0 {
                    break;
                }

                if !backoff.step() {
                    break;
                }
            }

            if index != !0 {
                break;
            }

            if default_index != !0 && deadline.is_none() {
                index = default_index;
                selected = 0;
                break;
            }

            // TODO: a test with send(foo(), msg) where foo is a FnOnce (and same for recv()).
            // TODO: call a hidden internal macro in order not to clutter the public one (`select_internal!`)

            // TODO: create a Handle/Select here.

            handle::current_reset();

            for case in &cases {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, _, _) = case;
                sel.promise(case_id);
            }

            for &(sel, _, _) in &cases {
                if !sel.is_blocked() {
                    handle::current_try_select(CaseId::abort());
                }
            }

            let timed_out = !handle::current_wait_until(deadline);
            let s = handle::current_selected();

            for case in &cases {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, _, _) = case;
                sel.revoke(case_id);
            }

            if timed_out {
                index = default_index;
                selected = 0;
                break;
            }

            if s != CaseId::abort() {
                for case in &cases {
                    let case_id = CaseId::new(case as *const _ as usize);
                    let &(sel, i, addr) = case;
                    if case_id == s {
                        if sel.fulfill(&mut token, &mut Backoff::new()) {
                            index = i;
                            selected = addr;
                            break;
                        }
                    }
                }

                if index != !0 {
                    break;
                }
            }
        }

        select_internal!(@finish token index selected $recv $send $default)

        // TODO: optimize TLS in selection (or even eliminate TLS, if possible?)
        // TODO: eliminate all thread-locals (mpsc doesn't use them!)
        // TODO: optimize send, try_recv, recv
        // TODO: allow recv(r) case

        // TODO: test select with duplicate cases - and make sure all of them fire (fariness)!

        // TODO: accept both Instant and Duration the in default case

        // TODO: test sending and receiving into the same channel from the same thread (all flavors)

        // TODO: allocate less memory in unbounded flavor if few elements are sent.
        // TODO: allocate memory lazily in unbounded flavor?
        // TODO: Run `cargo clippy` and make sure there are no warnings in here.

        // TODO: test init_single and init_multi
        // TODO: test with empty iterator
    }};

    (@container
        $cases:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
    ) => {
        use $crate::smallvec::SmallVec;
        let mut c: SmallVec<[_; 4]> = SmallVec::new();
        for r in $var.clone() {
            let addr = r as *const Receiver<_> as usize;
            c.push((r, $i, addr));
        }
        $cases = c;
    };
    (@container
        $cases:ident
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt,)
        ()
    ) => {
        use $crate::smallvec::SmallVec;
        let mut c: SmallVec<[_; 4]> = SmallVec::new();
        for s in $var.clone() {
            let addr = s as *const _ as usize;
            c.push((s, $i, addr));
        }
        $cases = c;
    };
    (@container
        $cases:ident
        $recv:tt
        $send:tt
    ) => {
        use $crate::smallvec::SmallVec;
        $cases = SmallVec::<[(&Sel<Token = Token>, usize, usize); 4]>::new();
        select_internal!(@push $cases $recv $send);
    };

    (@push
        $cases:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        for r in $var.clone() {
            let addr = r as *const Receiver<_> as usize;
            $cases.push((r, $i, addr));
        }
        select_internal!(@push $cases ($($tail)*) $send);
    };
    (@push
        $cases:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        for s in $var.clone() {
            let addr = s as *const _ as usize;
            $cases.push((s, $i, addr));
        }
        select_internal!(@push $cases () ($($tail)*));
    };
    (@push
        $cases:ident
        ()
        ()
    ) => {
    };

    (@default
        $deadline:ident
        $default_index:ident
        ()
    ) => {
        $deadline = None;
        $default_index = !0;
    };
    (@default
        $deadline:ident
        $default_index:ident
        (($i:tt $var:ident) default() => $body:tt,)
    ) => {
        $deadline = None;
        $default_index = $i;
    };
    (@default
        $deadline:ident
        $default_index:ident
        (($i:tt $var:ident) default($t:expr) => $body:tt,)
    ) => {
        $deadline = Some(Instant::now() + ($t));
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
            unsafe fn bind<'a, T: 'a, I>(_: &I, addr: usize) -> &'a Receiver<T>
            where
                I: Iterator<Item = &'a Receiver<T>>,
            {
                &*(addr as *const Receiver<T>)
            }
            let ($m, $r) = {
                let r = unsafe { bind(&$var, $selected) };
                let msg = unsafe { r.read(&mut $token) };
                r.finish(&mut $token);
                (msg, r)
            };
            $body
        } else {
            select_internal!(
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
            let $s = unsafe {
                struct Guard<F: FnMut()>(F);
                impl<F: FnMut()> Drop for Guard<F> {
                    fn drop(&mut self) {
                        self.0();
                    }
                }

                unsafe fn bind<'a, T: 'a, I>(_: &I, addr: usize) -> &'a T
                where
                    I: Iterator<Item = &'a T>,
                {
                    &*(addr as *const T)
                }

                let s = unsafe { bind(&$var, $selected) };

                let _msg = {
                    // We have to prefix variables with an underscore to get rid of warnings in
                    // case `$m` is of type `!`.
                    let _guard = Guard(|| s.fail(&mut $token));
                    let _msg = $m;

                    #[allow(unreachable_code)]
                    {
                        ::std::mem::forget(_guard);
                        _msg
                    }
                };

                s.write(&mut $token, _msg);
                s.finish(&mut $token);
                s
            };
            $body
        } else {
            select_internal!(
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
        (($i:tt $var:ident) default $args:tt => $body:tt,)
    ) => {
        if $index == $i {
            $body
        } else {
            select_internal!(
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
        unreachable!()
    };

    (@$($tail:tt)*) => {
        compile_error!(concat!("internal error in crossbeam-channel: ", stringify!(@$($tail)*)));
    };

    // The entry point.
    () => {
        compile_error!("empty block in `select_internal!`")
    };
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        select_internal!(@parse_list () ($($case $(($($args)*))* => $body,)*))
    };
    ($($tail:tt)*) => {
        select_internal!(@parse_list () ($($tail)*))
    };
}
