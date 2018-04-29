pub use self::case_id::CaseId;

mod case_id;

pub mod handle;

use smallvec::SmallVec;
use channel::{Sel, Token, PreparedSender, Receiver, Sender};
// use channel::SendLiteral;
pub type GenericContainer<'a> = SmallVec<[(&'a Sel<Token = Token>, usize); 4]>;

pub trait SelectArgument<'a> {
    type Container;
    fn init_single(&'a self, index: usize) -> Self::Container;
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>);
}

impl<'a, T: SelectArgument<'a>> SelectArgument<'a> for &'a T {
    type Container = <T as SelectArgument<'a>>::Container;
    fn init_single(&self, index: usize) -> Self::Container {
        (**self).init_single(index)
    }
    fn init_generic(&self, index: usize, c: &mut GenericContainer<'a>) {
        (**self).init_generic(index, c)
    }
}

impl<'a, T: 'a> SelectArgument<'a> for Receiver<T> {
    type Container = [(&'a Receiver<T>, usize); 1];
    fn init_single(&'a self, index: usize) -> Self::Container {
        [(self, index)]
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        c.push((self, index));
    }
}

impl<'a, T: 'a> SelectArgument<'a> for Sender<T> {
    type Container = [(&'a Sender<T>, usize); 1];
    fn init_single(&'a self, index: usize) -> Self::Container {
        [(self, index)]
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        c.push((self, index));
    }
}

impl<'a, T: 'a> SelectArgument<'a> for PreparedSender<'a, T> {
    type Container = [(&'a PreparedSender<'a, T>, usize); 1];
    fn init_single(&'a self, index: usize) -> Self::Container {
        [(self, index)]
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        c.push((self, index));
    }
}

impl<'a, T> SelectArgument<'a> for Option<&'a Receiver<T>> {
    type Container = SmallVec<[(&'a Receiver<T>, usize); 1]>;
    fn init_single(&'a self, index: usize) -> Self::Container {
        match *self {
            None => SmallVec::new(),
            Some(r) => SmallVec::from_buf([(r, index)])
        }
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        if let Some(r) = *self {
            c.push((r, index));
        }
    }
}

impl<'a, T> SelectArgument<'a> for Option<&'a Sender<T>> {
    type Container = SmallVec<[(&'a Sender<T>, usize); 1]>;
    fn init_single(&'a self, index: usize) -> Self::Container {
        match *self {
            None => SmallVec::new(),
            Some(s) => SmallVec::from_buf([(s, index)])
        }
    }
    fn init_generic(&'a self, index: usize, c: &mut GenericContainer<'a>) {
        if let Some(s) = *self {
            c.push((s, index));
        }
    }
}

#[macro_export]
macro_rules! select {
    // Success! The list is empty.
    (@parse_list ($($head:tt)*) ()) => {
        select!(
            @parse_case
            ()
            ()
            ()
            ($($head)*)
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
            )
        )
    };
    // If necessary, insert an empty argument list after `default`.
    (@parse_list
        ($($head:tt)*)
        (default => $($tail:tt)*)
    ) => {
        select!(@parse_list ($($head)*) (default() => $($tail)*))
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
    (@parse_list ($($head:tt)*) ($($tail:tt)*)) => {
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

    // Success! All cases were consumed.
    (@parse_case
        $recv:tt
        $send:tt
        $default:tt
        ()
        $labels:tt
    ) => {
        select!(@generate $recv $send $default)
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
        (recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
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
        (recv($r:expr, $m:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        select!(
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
        select!(
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
        select!(
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
        select!(
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
        select!(
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
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    (@parse_case
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default($t:expr) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    (@parse_case
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default($t:expr,) => $body:tt, $($tail:tt)*)
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

    (@generate $recv:tt $send:tt $default:tt) => {{
        // TODO: Remove all these imports to avoid the "unused import" warnings.
        use $crate::select::CaseId;
        use $crate::channel::Sel;
        use $crate::select::handle;
        use std::time::Instant;
        use $crate::utils::{shuffle, Backoff};
        use $crate::channel::Token;
        use $crate::select::{SelectArgument};

        let deadline: Option<Instant>;
        let default_index: usize;
        select!(@default deadline default_index $default);

        let mut token: Token = unsafe { ::std::mem::zeroed() };
        let mut index: usize = !0;

        // TODO: #[allow(warnings)]
        select!(@declare $recv);
        select!(@declare $send);
        let mut cases;
        select!(@container cases $recv $send);
        shuffle(&mut cases);

        loop {
            // TODO: Tune backoff for zero flavor performance (too much yielding is bad)
            let backoff = &mut Backoff::new();
            loop {
                for &(sel, i) in &cases {
                    if sel.try(&mut token, backoff) {
                        index = i;
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
                break;
            }

            // TODO: a test with send(foo(), msg) where foo is a FnOnce (and same for recv()).
            // TODO: call a hidden internal macro in order not to clutter the public one (`select!`)

            handle::current_reset();

            for case in &cases {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, _) = case;
                sel.promise(case_id);
            }

            for &(sel, _) in &cases {
                if !sel.is_blocked() {
                    handle::current_try_select(CaseId::abort());
                }
            }

            let timed_out = !handle::current_wait_until(deadline);
            let s = handle::current_selected();

            for case in &cases {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, _) = case;
                sel.revoke(case_id);
            }

            if timed_out {
                index = default_index;
                break;
            }

            if s != CaseId::abort() {
                for case in &cases {
                    let case_id = CaseId::new(case as *const _ as usize);
                    let &(sel, i) = case;
                    if case_id == s {
                        if sel.fulfill(&mut token, &mut Backoff::new()) {
                            index = i;
                            break;
                        }
                    }
                }

                if index != !0 {
                    break;
                }
            }
        }

        select!(@finish index token $recv $send $default)

        // TODO: to be consistent, `select! { recv(r, _) => () }` should move `r`, not borrow!
        // TODO: - or maybe borrow in both single and multi cases?
        // TODO: we should be able to pass in `Box<Receiver<T>>` and `Box<Option<Receiver<T>>`
        // TODO: - or maybe `Option<Box<Receiver<T>>>`?

        // TODO: optimize TLS in selection (or even eliminate TLS, if possible?)
        // TODO: eliminate all thread-locals (mpsc doesn't use them!)

        // TODO: test select with duplicate cases

        // TODO: accept both Instant and Duration the in default case

        // TODO: test sending and receiving into the same channel from the same thread (all flavors)

        // TODO: allocate less memory in unbounded flavor if few elements are sent.
        // TODO: allocate memory lazily in unbounded flavor?
        // TODO: Run `cargo clippy` and make sure there are no warnings in here.
    }};

    (@declare ($(($i:tt $var:ident) $method:tt ($v:expr, $($args:tt)*) => $body:tt,)*)) => {
        $(
            let $var = &($v);
        )*
    };

    (@container
        $cases:ident
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt,)
        ()
    ) => {
        $cases = SelectArgument::init_single($var, $i);
    };
    (@container
        $cases:ident
        ()
        (($i:tt $var:ident) send($s:expr, $m:expr) => $body:tt,)
    ) => {
        $cases = SelectArgument::init_single($var, $i);
    };
    (@container
        $cases:ident
        $recv:tt
        $send:tt
    ) => {
        $cases = $crate::select::GenericContainer::new();
        select!(@push $cases $recv $send);
    };

    (@push
        $cases:ident
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        $send:tt
    ) => {
        SelectArgument::init_generic($var, $i, &mut $cases);
        select!(@push $cases ($($tail)*) $send);
    };
    (@push
        $cases:ident
        ()
        (($i:tt $var:ident) send($s:expr, $m:expr) => $body:tt, $($tail:tt)*)
    ) => {
        SelectArgument::init_generic($var, $i, &mut $cases);
        select!(@push $cases () ($($tail)*));
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
        $index:ident
        $token:ident
        (($i:tt $var:ident) recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        $send:tt
        $default:tt
    ) => {
        if $index == $i {
            let $m = {
                let msg = unsafe { $var.read(&mut $token) };
                $var.finish(&mut $token);
                msg
            };
            $body
        } else {
            select!(
                @finish
                $index
                $token
                ($($tail)*)
                $send
                $default
            )
        }
    };
    (@finish
        $index:ident
        $token:ident
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

                let msg = {
                    let guard = Guard(|| $var.fail(&mut $token));
                    let msg = $m;
                    ::std::mem::forget(guard);
                    msg
                };

                $var.write(&mut $token, msg);
                $var.finish(&mut $token);
            }
            $body
        } else {
            select!(
                @finish
                $index
                $token
                ()
                ($($tail)*)
                $default
            )
        }
    };
    (@finish
        $index:ident
        $token:ident
        ()
        ()
        (($i:tt $var:ident) default $args:tt => $body:tt,)
    ) => {
        if $index == $i {
            $body
        } else {
            select!(
                @finish
                $index
                $token
                ()
                ()
                ()
            )
        }
    };
    (@finish
        $index:ident
        $token:ident
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
        compile_error!("empty block in `select!`")
    };
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        select!(@parse_list () ($($case $(($($args)*))* => $body,)*))
    };
    ($($tail:tt)*) => {
        select!(@parse_list () ($($tail)*))
    };
}
