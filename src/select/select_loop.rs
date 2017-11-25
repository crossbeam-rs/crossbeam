/// This macro provides a more user-friendly wrapper around the `Select` struct.
///
/// It is similar in syntax to a `match` expression: It takes a list of cases,
/// each of the form `operation(arguments) => expression`. Note that
/// `expression` may be a block as well. The individual cases may optionally be
/// separated by commas. As with `match`, the whole macro invocation is treated
/// as an expression, so values can be returned from the individual cases.
///
/// A simple example to illustrate:
///
/// ```ignore
/// select_loop! {
///     send(tx0, value) => println!("Sent value to tx0!"),
///     send(tx1, value) => { println!("Sent value to tx1!") }
/// }
/// ```
///
/// Please keep in mind the standard rules of the `Select`:
///
/// 1. No two cases may use the same end of the same channel. In the example
///    above this means that even if `tx0` and `tx1` are different `Sender`
///    instances, they must not belong to the same channel.
///
/// The following operations are supported by the macro:
///
/// * **`send($tx:expr, $variable:ident) => ...`:** Attempt to `send` the value of
///   `$variable` with `$tx`. If sending fails, the value of `$variable` is
///   automatically recovered. For convenience, `$variable` is automatically
///   made mutable by the macro.
/// * **`send($tx:expr, $expr:expr) => ...`:** Attempt to `send` the result of
///   evaluating `$expr` with `$tx`. *Warning:* $expr is evaluated once in every
///   loop iteration. In some iterations, its result may simply be discarded
///   with no actual attempt at sending it.
/// * **`send($tx:expr, mut $field:expr) => ...`:** Attempt to `send` the value of
///   `$variable` with `$tx`. If sending fails, the value of `$variable` is
///   automatically recovered. Different from the previous version, this version
///   assumes that `$field` is mutable. And while the macro accepts any
///   expression for `$field`, it only really makes sense to specify plain
///   variables or direct field references (e.g. `some_struct.some_field`).
/// * **`recv($rx:expr, $variable:ident) => ...`:** Attempt to receive a value from
///   `$rx`. If successful, the received value will be available in `$variable`
///    for the expression of this case.
/// * **`recv($rx:expr, mut $variable:ident) => ...`:** Attempt to receive a value
///   from `$rx`. If successful, the received value will be available in the
///   mutable `$variable` for the expression of this case.
/// * **`recv($rx:expr, _) => ...`:** Attempt to receive a value
///   from `$rx`. If successful, the received value is discarded.
/// * **`disconnected() => ...`:** This case is triggered when all channels in the
///   loop have disconnected.
/// * **`would_block() => ...`:** This case is triggered when all channels in the
///   loop would block.
/// * **`timed_out($duration:expr) => ...`:** Enables a timeout for the select loop.
///   If the timeout is reached without any of the other cases triggering, this
///   case is triggered. If there are multiple `timed_out` cases, only the
///   duration from the last case is registered.
///
#[macro_export]
macro_rules! select_loop {
    // The main entrypoint
    {$($method:ident($($args:tt)*) => $body:expr$(,)*)*} => {
        // On stable Rust, 'unused_mut' warnings have to be suppressed within the whole block.
        #[cfg_attr(not(feature = "nightly"), allow(unused_mut))]
        {
            #[allow(unused_mut, unused_variables)]
            let mut state = $crate::Select::new();

            // Build the prelude.
            //
            // This currently performs two tasks:
            //
            // 1) Make all variables used in send(_, $ident) mutable (the
            //    variable will move into the loop anyway, so we can move it
            //    here as well).
            // 2) If a timeout was specified, overwrite the above state with one
            //    which has the timeout set.
            $(select_loop!(@prelude(state) $method($($args)*));)*

            // The actual select loop which a user would write manually
            loop {
                #[allow(bad_style)]
                struct _DONT_USE_AN_UNLABELED_BREAK_IN_SELECT_LOOP;

                $(
                    // Build the actual method invocations.
                    select_loop! {
                        @impl(state)
                        $method($($args)*) => {
                            // This double-loop construct is used to guard against the user
                            // using unlabeled breaks and continues in their code.
                            // It works by abusing Rust's control flow analysis.
                            //
                            // If the user code (`$body`) contains an unlabeled break, the
                            // inner loop will be broken with a result whose type doesn't match
                            // `_DONT_USE_AN_UNLABELED_BREAK_IN_SELECT_LOOP`, and that
                            // will show up in error messages.
                            //
                            // Similarly, if the user code contains an unlabeled continue,
                            // the inner loop will try to assign a value to variable
                            // `_DONT_USE_AN_UNLABELED_CONTINUE_IN_SELECT_LOOP` twice, which
                            // will again show up in error messages.
                            #[allow(bad_style)]
                            let _DONT_USE_AN_UNLABELED_CONTINUE_IN_SELECT_LOOP;

                            #[allow(unused_variables)]
                            let res;

                            #[allow(unreachable_code)]
                            let _: _DONT_USE_AN_UNLABELED_BREAK_IN_SELECT_LOOP = loop {
                                _DONT_USE_AN_UNLABELED_CONTINUE_IN_SELECT_LOOP = ();
                                res = $body;
                                break _DONT_USE_AN_UNLABELED_BREAK_IN_SELECT_LOOP;
                            };
                            break res;
                        }
                    }
                )*
            }
        }
    };

    // The individual method invocations
    {@impl($state:ident) send($tx:expr, $val:ident) => $body:expr} => {
        match $state.send(&*&$tx, $val) {
            Ok(()) => {
                $body
            }
            Err($crate::SelectSendError(val)) => {
                $val = val;
            }
        }
    };
    {@impl($state:ident) send($tx:expr, $val:ident) => $body:expr} => {
        if let Ok(()) = $state.send(&*&$tx, $val) {
            $body
        }
    };
    {@impl($state:ident) send($tx:expr, mut $val:expr) => $body:expr} => {
        match $state.send(&*&$tx, $val) {
            Ok(()) => {
                $body
            }
            Err($crate::SelectSendError(val)) => {
                $val = val;
            }
        }
    };
    {@impl($state:ident) send($tx:expr, $val:expr) => $body:expr} => {
        if let Ok(()) = $state.send(&*&$tx, $val) {
            $body
        }
    };
    {@impl($state:ident) recv($rx:expr, _) => $body:expr} => {
        if let Ok(_) = $state.recv(&*&$rx) {
            $body
        }
    };
    {@impl($state:ident) recv($rx:expr, $val:ident) => $body:expr} => {
        if let Ok($val) = $state.recv(&*&$rx) {
            $body
        }
    };
    {@impl($state:ident) recv($rx:expr, mut $val:ident) => $body:expr} => {
        if let Ok(mut $val) = $state.recv(&*&$rx) {
            $body
        }
    };
    {@impl($state:ident) disconnected() => $body:expr} => {
        if $state.disconnected() {
            $body
        }
    };
    {@impl($state:ident) would_block() => $body:expr} => {
        if $state.would_block() {
            $body
        }
    };
    {@impl($state:ident) timed_out($_timeout:expr) => $body:expr} => {
        if $state.timed_out() {
            $body
        }
    };

    // The prelude helpers
    {@prelude($state:ident) send($tx:expr, $val:ident)} => {
        #[allow(unused_mut, unused_variables)]
        let mut $val = $val;
    };
    {@prelude($state:ident) timed_out($timeout:expr)} => {
        #[allow(unused_mut, unused_variables)]
        let mut $state = $crate::Select::with_timeout($timeout);
    };
    {@prelude($state:ident) $($tail:tt)*} => {};
}
