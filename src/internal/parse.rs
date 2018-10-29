//! Parser for the `select!` macro.

/// Parses the contents of the `select!` macro and passes the result to the `$callback` macro.
///
/// If there is a syntax error, fails with a compile-time error.
///
/// Otherwise, it parses the macro into two lists and passes them to the `$callback` macro. The
/// first one is a comma-separated list of receive and send cases, while the second one is either
/// empty or contains the single default case.
///
/// Each send and receive is of the form `case(arguments) -> result => block`, where:
/// - `case` is either `recv` or `send`
/// - `arguments` is a list of arguments
/// - `result` is a pattern bound to the result of the operation
///
/// The `default` case, if present, takes one of these two forms:
/// - `default() => block`
/// - `default(timeout) => block`
///
/// Both lists, if not empty, have a trailing comma at the end.
///
/// For example, this invocation of `select!`:
///
/// ```ignore
/// select! {
///     recv(a) -> res1 => x,
///     recv(b) -> res2 => y,
///     send(c, msg) -> res3 => { z }
///     default => {}
/// }
/// ```
///
/// Would be parsed as:
///
/// ```ignore
/// (
///   recv(a) -> res1 => { x },
///   recv(b) -> res2 => { y },
///   send(c, msg) -> res3 => { { z } },
/// )
/// (
///   default() => { {} },
/// )
/// ```
///
/// These two lists are then passed to `$callback` as two arguments.
#[doc(hidden)]
#[macro_export]
macro_rules! __crossbeam_channel_parse {
    // Success! The list is empty.
    // Now check the arguments of each processed case.
    (@list
        $callback:ident
        ($($head:tt)*)
        ()
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($head)*)
            ()
            ()
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
    // But print an error if `default` is followed by a `->`.
    (@list
        $callback:ident
        ($($head:tt)*)
        (default -> $($tail:tt)*)
    ) => {
        compile_error!("expected `=>` after the `default` case, found `->`")
    };
    // Print an error if there's an `->` after the argument list in the `default` case.
    (@list
        $callback:ident
        ($($head:tt)*)
        (default $args:tt -> $($tail:tt)*)
    ) => {
        compile_error!("expected `=>` after the `default` case, found `->`")
    };
    // Print an error if there is a missing result in a `recv` case.
    (@list
        $callback:ident
        ($($head:tt)*)
        (recv($($args:tt)*) => $($tail:tt)*)
    ) => {
        compile_error!("expected `->` after the `recv` case, found `=>`")
    };
    // Print an error if there is a missing result in a `send` case.
    (@list
        $callback:ident
        ($($head:tt)*)
        (send($($args:tt)*) => $($tail:tt)*)
    ) => {
        compile_error!("expected `->` after the `send` case, found `=>`")
    };
    // Make sure the arrow and the result are not repeated.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt -> $res:tt -> $($tail:tt)*)
    ) => {
        compile_error!("expected `=>`, found `->`")
    };
    // Print an error if there is a semicolon after the block.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident $args:tt $(-> $res:pat)* => $body:block; $($tail:tt)*)
    ) => {
        compile_error!("did you mean to put a comma instead of the semicolon after `}`?")
    };
    // The first case is separated by a comma.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:expr, $($tail:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
            ($($tail)*)
        )
    };
    // Don't require a comma after the case if it has a proper block.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:block $($tail:tt)*)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
            ($($tail)*)
        )
    };
    // Only one case remains.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:expr)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
            ()
        )
    };
    // Accept a trailing comma at the end of the list.
    (@list
        $callback:ident
        ($($head:tt)*)
        ($case:ident ($($args:tt)*) $(-> $res:pat)* => $body:expr,)
    ) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ($($head)* $case ($($args)*) $(-> $res)* => { $body },)
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
    (@list_error3 $case:ident($($args:tt)*) $(-> $r:pat)*) => {
        compile_error!(concat!(
            "missing `=>` after the `",
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
        __crossbeam_channel_parse!(@list_error4 $($tail)*)
    };
    // Stage 4: fail with a generic error message.
    (@list_error4 $($tail:tt)*) => {
        compile_error!("invalid syntax")
    };

    // Success! All cases were parsed.
    (@case
        $callback:ident
        ()
        ($($cases:tt)*)
        $default:tt
    ) => {
        $callback!(
            ($($cases)*)
            $default
        )
    };

    // Check the format of a `recv` case...
    (@case
        $callback:ident
        (recv($r:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            ($($cases)* recv($r) -> $res => $body,)
            $default
        )
    };
    // Allow trailing comma...
    (@case
        $callback:ident
        (recv($r:expr,) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            ($($cases)* recv($r) -> $res => $body,)
            $default
        )
    };
    // Error cases...
    (@case
        $callback:ident
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
        $callback:ident
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
        $callback:ident
        (send($s:expr, $m:expr) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            ($($cases)* send($s, $m) -> $res => $body,)
            $default
        )
    };
    // Allow trailing comma...
    (@case
        $callback:ident
        (send($s:expr, $m:expr,) -> $res:pat => $body:tt, $($tail:tt)*)
        ($($cases:tt)*)
        $default:tt
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            ($($cases)* send($s, $m) -> $res => $body,)
            $default
        )
    };
    // Error cases...
    (@case
        $callback:ident
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
        $callback:ident
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
        $callback:ident
        (default() => $body:tt, $($tail:tt)*)
        $cases:tt
        ()
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            $cases
            (default() => $body,)
        )
    };
    // Check the format of a `default` case with timeout.
    (@case
        $callback:ident
        (default($timeout:expr) => $body:tt, $($tail:tt)*)
        $cases:tt
        ()
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            $cases
            (default($timeout) => $body,)
        )
    };
    // Allow trailing comma...
    (@case
        $callback:ident
        (default($timeout:expr,) => $body:tt, $($tail:tt)*)
        $cases:tt
        ()
    ) => {
        __crossbeam_channel_parse!(
            @case
            $callback
            ($($tail)*)
            $cases
            (default($timeout) => $body,)
        )
    };
    // Check for duplicate default cases...
    (@case
        $callback:ident
        (default $($tail:tt)*)
        $cases:tt
        ($($def:tt)+)
    ) => {
        compile_error!("there can be only one `default` case in a `select!` block")
    };
    // Other error cases...
    (@case
        $callback:ident
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
        $callback:ident
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
        $callback:ident
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

    // Catches a bug within this macro (should not happen).
    (@$($tokens:tt)*) => {
        compile_error!(concat!(
            "internal error in crossbeam-channel: ",
            stringify!(@$($tokens)*),
        ))
    };

    // The entry points.
    ($($case:ident $(($($args:tt)*))* => $body:expr $(,)*)*) => {
        __crossbeam_channel_parse!(
            @list
            $callback
            ()
            ($($case $(($($args)*))* => $body,)*)
        )
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
