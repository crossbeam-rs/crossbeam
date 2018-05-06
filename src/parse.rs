// TODO: explain input and output, and error reporting
#[macro_export]
#[doc(hidden)]
macro_rules! __crossbeam_channel_parse {
    // TODO: place entry point at the end

    // Success! The list is empty.
    (@list
        $callback:ident
        ($($head:tt)*)
        ()
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
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
    (@list
        $callback:ident
        ($($head:tt)*)
        (default => $($tail:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)*)
            (default() => $($tail)*)
        )
    };
    // The first case is separated by a comma.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr, $($tail:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case $args => { $body },)
            ($($tail)*)
        )
    };
    // Print an error if there is a semicolon after the block.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt => $body:block; $($tail:tt)*)
    ) => {
        compile_error!("did you mean to put a comma instead of the semicolon after `}`?")
    };
    // Don't require a comma after the case if it has a proper block.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt => $body:block $($tail:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case $args => { $body },)
            ($($tail)*)
        )
    };
    // Only one case remains.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case $args => { $body },)
            ()
        )
    };
    // Accept a trailing comma at the end of the list.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt => $body:expr,)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case $args => { $body },)
            ()
        )
    };
    // Diagnose and print an error.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($($tail:tt)*)
    ) => {
        __crossbeam_channel_parse!(@list_error1 $($tail)*)
    };
    // Stage 1: check the case type.
    (@list_error1 recv $($tail:tt)*) => {
        __crossbeam_channel_parse!(@list_error2 recv $($tail)*)
    };
    (@list_error1 send $($tail:tt)*) => {
        __crossbeam_channel_parse!(@list_error2 send $($tail)*)
    };
    (@list_error1 default $($tail:tt)*) => {
        __crossbeam_channel_parse!(@list_error2 default $($tail)*)
    };
    (@list_error1 $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected one of `recv`, `send`, or `default`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error1 $($tail:tt)*) => {
        __crossbeam_channel_parse!(@list_error2 $($tail)*);
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
        __crossbeam_channel_parse!(@list_error3 $($tail)*)
    };
    // Stage 3: check the `=>` and what comes after it.
    (@list_error3 $case:ident($($args:tt)*)) => {
        compile_error!(concat!(
            "missing `=>` after the argument list of `",
            stringify!($case),
            "`",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) =>) => {
        compile_error!("expected expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) => $body:expr; $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma instead of the semicolon after `",
            stringify!($body),
            "`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) => recv($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) => send($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) => default($($a:tt)*) $($tail:tt)*) => {
        compile_error!("expected an expression after `=>`")
    };
    (@list_error3 $case:ident($($args:tt)*) => $f:ident($($a:tt)*) $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "(",
            stringify!($($a)*),
            ")`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) => $f:ident!($($a:tt)*) $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "!(",
            stringify!($($a)*),
            ")`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) => $f:ident![$($a:tt)*] $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "![",
            stringify!($($a)*),
            "]`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) => $f:ident!{$($a:tt)*} $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($f),
            "!{",
            stringify!($($a)*),
            "}`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) => $body:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma after `",
            stringify!($body),
            "`?",
        ))
    };
    (@list_error3 $case:ident($($args:tt)*) $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected `=>`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@list_error3 $case:ident $args:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected an argument list, found `",
            stringify!($args),
            "`",
        ))
    };
    (@list_error3 $($tail:tt)*) => {
        __crossbeam_channel_parse!(@list_error4 $($tail)*)
    };
    // Stage 4: fail with a generic error message.
    (@list_error4 $($tail:tt)*) => {
        compile_error!("invalid syntax")
    };

    // Success! All cases were consumed.
    (@case
        $callback:ident
        ($($recv:tt)*)
        ($($send:tt)*)
        $default:tt
        ()
        $labels:tt
    ) => {
        $callback!(
            ($($recv)*)
            ($($send)*)
            $default
        )
    };
    // Error: there are no labels left.
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        $default:tt
        $cases:tt
        ()
    ) => {
        compile_error!("too many cases in a `select!` block")
    };
    // Check the format of a `recv` case...
    (@case
        $callback:ident
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr, $m:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($recv)* $label recv(&$r, $m, _) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@case
        $callback:ident
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($rs:expr, $m:pat, $r:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($recv)* $label recv($rs, $m, $r) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Allow trailing comma...
    (@case
        $callback:ident
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($r:expr, $m:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($recv)* $label recv($r, $m, _) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@case
        $callback:ident
        ($($recv:tt)*)
        $send:tt
        $default:tt
        (recv($rs:expr, $m:pat, $r:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($recv)* $label recv($rs, $m, $r) => $body,)
            $send
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Error cases...
    (@case
        $callback:ident
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
    (@case
        $callback:ident
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
    (@case
        $callback:ident
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($s:expr, $m:expr) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            ($($send)* $label send($s, $m, _) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@case
        $callback:ident
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            ($($send)* $label send($ss, $m, $s) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Allow trailing comma...
    (@case
        $callback:ident
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($s:expr, $m:expr,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            ($($send)* $label send($s, $m, _) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@case
        $callback:ident
        $recv:tt
        ($($send:tt)*)
        $default:tt
        (send($ss:expr, $m:expr, $s:pat,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            ($($send)* $label send($ss, $m, $s) => $body,)
            $default
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Error cases...
    (@case
        $callback:ident
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
    (@case
        $callback:ident
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
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        ()
        (default() => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            $send
            ($label default() => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        ()
        (default($t:expr) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            $send
            ($label default($t) => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Allow trailing comma...
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        ()
        (default($t:expr,) => $body:tt, $($tail:tt)*)
        ($label:tt $($labels:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            $recv
            $send
            ($label default($t) => $body,)
            ($($tail)*)
            ($($labels)*)
        )
    };
    // Valid, but duplicate cases...
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default() => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default($t:expr) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    (@case
        $callback:ident
        $recv:tt
        $send:tt
        ($($default:tt)+)
        (default($t:expr,) => $body:tt, $($tail:tt)*)
        $labels:tt
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    // Other error cases...
    (@case
        $callback:ident
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
    (@case
        $callback:ident
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
    (@case
        $callback:ident
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

    (@$($tokens:tt)*) => {
        compile_error!(concat!("internal error in crossbeam-channel: ", stringify!(@$($tokens)*)));
    };

    // The entry point.
    ($callback:ident) => {
        compile_error!("empty block in `select!`")
    };
    ($callback:ident $($tokens:tt)*) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ()
            ($($tokens)*)
        )
    };
}
