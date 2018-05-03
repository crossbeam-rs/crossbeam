pub use self::case_id::CaseId;

mod case_id;

pub mod handle;

use smallvec::SmallVec;
use channel::{Token, PreparedSender, Receiver, Sender};
use utils::Backoff;

#[macro_export]
macro_rules! select {
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        select_internal!(@parse_list () ($($case $(($($args)*))* => $body,)*))
    };
    ($($tokens:tt)*) => {
        select_internal!(@parse_list () ($($tokens)*))
    };
}

pub trait SelectArgument<'a, T> {
    fn extract(&'a self) -> &'a T;
}
impl<'a, T, A: SelectArgument<'a, T>> SelectArgument<'a, T> for &'a A {
    fn extract(&'a self) -> &'a T {
        (**self).extract()
    }
}
impl<'a, T> SelectArgument<'a, Receiver<T>> for Receiver<T> {
    fn extract(&'a self) -> &'a Receiver<T> {
        self
    }
}
impl<'a, T: 'a> SelectArgument<'a, Receiver<T>> for Option<&'a Receiver<T>> {
    fn extract(&'a self) -> &'a Receiver<T> {
        self.unwrap()
    }
}
impl<'a, T> SelectArgument<'a, Sender<T>> for Sender<T> {
    fn extract(&'a self) -> &'a Sender<T> {
        self
    }
}
impl<'a, T: 'a> SelectArgument<'a, Sender<T>> for Option<&'a Sender<T>> {
    fn extract(&'a self) -> &'a Sender<T> {
        self.unwrap()
    }
}
impl<'a, T> SelectArgument<'a, PreparedSender<'a, T>> for PreparedSender<'a, T> {
    fn extract(&'a self) -> &'a PreparedSender<'a, T> {
        self
    }
}
impl<'a, T: 'a> SelectArgument<'a, PreparedSender<'a, T>> for Option<&'a PreparedSender<'a, T>> {
    fn extract(&'a self) -> &'a PreparedSender<'a, T> {
        self.unwrap()
    }
}

pub type GenericContainer<'a> = SmallVec<[(&'a Sel<Token = Token>, usize, usize); 4]>;

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

pub trait RecvArgument<'a> {
    type Container;
    fn init_single(&'a self, index: usize) -> Self::Container;
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>);
}

impl<'a, T: RecvArgument<'a>> RecvArgument<'a> for &'a T {
    type Container = <T as RecvArgument<'a>>::Container;
    fn init_single(&self, index: usize) -> Self::Container {
        (**self).init_single(index)
    }
    fn init_generic(&self, index: usize, c: &mut GenericContainer<'a>) {
        (**self).init_generic(index, c)
    }
}

impl<'a, T: 'a> RecvArgument<'a> for Receiver<T> {
    type Container = [(&'a Receiver<T>, usize, usize); 1];
    fn init_single(&'a self, index: usize) -> Self::Container {
        [(self, index, self as *const Receiver<T> as usize)]
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        c.push((self, index, self as *const Receiver<T> as usize));
    }
}

impl<'a, T> RecvArgument<'a> for Option<&'a Receiver<T>> {
    type Container = SmallVec<[(&'a Receiver<T>, usize, usize); 1]>;
    fn init_single(&'a self, index: usize) -> Self::Container {
        match *self {
            None => SmallVec::new(),
            Some(r) => SmallVec::from_buf([(r, index, r as *const Receiver<T> as usize)])
        }
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        if let Some(r) = *self {
            c.push((r, index, r as *const Receiver<T> as usize));
        }
    }
}

pub trait SendArgument<'a> {
    type Container;
    fn init_single(&'a self, index: usize) -> Self::Container;
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>);
}

impl<'a, T: SendArgument<'a>> SendArgument<'a> for &'a T {
    type Container = <T as SendArgument<'a>>::Container;
    fn init_single(&self, index: usize) -> Self::Container {
        (**self).init_single(index)
    }
    fn init_generic(&self, index: usize, c: &mut GenericContainer<'a>) {
        (**self).init_generic(index, c)
    }
}

impl<'a, T: 'a> SendArgument<'a> for Sender<T> {
    type Container = [(&'a Sender<T>, usize, usize); 1];
    fn init_single(&'a self, index: usize) -> Self::Container {
        [(self, index, self as *const Sender<T> as usize)]
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        c.push((self, index, self as *const Sender<T> as usize));
    }
}

impl<'a, T: 'a> SendArgument<'a> for PreparedSender<'a, T> {
    type Container = [(&'a PreparedSender<'a, T>, usize, usize); 1];
    fn init_single(&'a self, index: usize) -> Self::Container {
        [(self, index, self as *const PreparedSender<T> as usize)]
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        c.push((self, index, self as *const PreparedSender<T> as usize));
    }
}

impl<'a, T> SendArgument<'a> for Option<&'a Sender<T>> {
    type Container = SmallVec<[(&'a Sender<T>, usize, usize); 1]>;
    fn init_single(&'a self, index: usize) -> Self::Container {
        match *self {
            None => SmallVec::new(),
            Some(s) => SmallVec::from_buf([(s, index, s as *const Sender<T> as usize)])
        }
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        if let Some(s) = *self {
            c.push((s, index, s as *const Sender<T> as usize));
        }
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
            ($($recv)* $label recv($r, $m) => $body,)
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
            ($($recv)* $label recv($r, $m) => $body,)
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
            ($($send)* $label send($s, $m) => $body,)
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
            ($($send)* $label send($s, $m) => $body,)
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
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        match &$r {
            $var => {
                use $crate::select::SelectArgument;
                let _: &SelectArgument<_> = &$var;
                select_internal!(
                    @declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    };
    (@declare
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        match &mut ($rs).into_iter() {
            $var => {
                let _: &Iterator<Item = &Receiver<_>> = &$var;
                select_internal!(
                    @declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    };
    (@declare
        (($i:tt $var:ident) send($s:expr, $m:expr) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        match &$s {
            $var => {
                use $crate::select::SelectArgument;
                let _: &SelectArgument<_> = &$var;
                select_internal!(
                    @declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
            }
        }
    };
    (@declare
        (($i:tt $var:ident) send($ss:expr, $m:pat, $s:pat) => $body:tt, $($tail:tt)*)
        $recv:tt
        $send:tt
        $default:tt
    ) => {
        match &mut ($ss).into_iter() {
            $var => {
                let _: &Iterator<Item = &Sender<_>> = &$var;
                select_internal!(
                    @declare
                    ($($tail)*)
                    $recv
                    $send
                    $default
                )
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
        use $crate::select::RecvArgument;
        use $crate::select::SendArgument;
        use $crate::select::SelectArgument;

        use $crate::channel::Token;
        use $crate::select::CaseId;
        use $crate::select::handle;
        use $crate::utils::{Backoff, shuffle};

        let deadline: Option<Instant>;
        let default_index: usize;
        select_internal!(@default deadline default_index $default);

        let mut cases;
        select_internal!(@container cases $recv $send);
        shuffle(&mut cases);

        let mut token: Token = unsafe { ::std::mem::zeroed() };
        let mut index: usize = !0;
        let mut selected: usize = 0;

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

        // TODO: test select with duplicate cases

        // TODO: accept both Instant and Duration the in default case

        // TODO: test sending and receiving into the same channel from the same thread (all flavors)

        // TODO: allocate less memory in unbounded flavor if few elements are sent.
        // TODO: allocate memory lazily in unbounded flavor?
        // TODO: Run `cargo clippy` and make sure there are no warnings in here.

        // TODO: test init_single and init_multi
    }};

    (@container
        $cases:ident
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt,)
        ()
    ) => {
        $cases = RecvArgument::init_single($var, $i);
    };
    (@container
        $cases:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
    ) => {
        use $crate::smallvec::SmallVec;
        let mut c: SmallVec<[_; 4]> = SmallVec::new();
        while let Some(r) = $var.next() {
            let addr = r as *const Receiver<_> as usize;
            c.push((r, $i, addr));
        }
        $cases = c;
    };
    (@container
        $cases:ident
        ()
        (($i:tt $var:ident) send($s:expr, $m:expr) => $body:tt,)
    ) => {
        $cases = SendArgument::init_single($var, $i);
    };
    (@container
        $cases:ident
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt,)
        ()
    ) => {
        use $crate::smallvec::SmallVec;
        let mut c: SmallVec<[_; 4]> = SmallVec::new();
        while let Some(r) = $var.next() {
            let addr = r as *const Receiver<_> as usize;
            c.push((r, $i, addr));
        }
        $cases = c;
    };
    (@container
        $cases:ident
        $recv:tt
        $send:tt
    ) => {
        $cases = $crate::select::GenericContainer::new();
        select_internal!(@push $cases $recv $send);
    };

    (@push
        $cases:ident
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        RecvArgument::init_generic($var, $i, &mut $cases);
        select_internal!(@push $cases ($($tail)*) $send);
    };
    (@push
        $cases:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        while let Some(r) = $var.next() {
            let addr = r as *const Receiver<_> as usize;
            $cases.push((r, $i, addr));
        }
        select_internal!(@push $cases ($($tail)*) $send);
    };
    (@push
        $cases:ident
        ()
        (($i:tt $var:ident) send($s:expr, $m:expr) => $body:tt, $($tail:tt)*)
    ) => {
        SendArgument::init_generic($var, $i, &mut $cases);
        select_internal!(@push $cases () ($($tail)*));
    };
    (@push
        $cases:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        while let Some(s) = $var.next() {
            let addr = s as *const Sender<_> as usize;
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
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        $send:tt
        $default:tt
    ) => {
        if $index == $i {
            let $m = {
                let r = SelectArgument::extract($var);
                let msg = unsafe { r.read(&mut $token) };
                r.finish(&mut $token);
                msg
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
        (($i:tt $var:ident) send($s:expr, $m:expr) => $body:tt, $($tail:tt)*)
        $default:tt
    ) => {
        if $index == $i {
            unsafe {
                struct Guard<F: FnMut()>(F);
                impl<F: FnMut()> Drop for Guard<F> {
                    fn drop(&mut self) {
                        self.0();
                    }
                }

                let s = SelectArgument::extract($var);
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
            }
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

                unsafe fn bind<'a, T: 'a, I>(_: &I, addr: usize) -> &'a Sender<T>
                where
                    I: Iterator<Item = &'a Sender<T>>,
                {
                    &*(addr as *const Sender<T>)
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
