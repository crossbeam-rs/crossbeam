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
/// * **`send($tx:expr, mut $field:expr) => ...`:** Attempt to `send` the value of
///   `$variable` with `$tx`. If sending fails, the value of `$variable` is
///   automatically recovered. Different from the previous version, this version
///   assumes that `$field` is mutable. And while the macro accepts any
///   expression for `$field`, it only really makes sense to specify plain
///   variables or direct field references (e.g. `some_struct.some_field`).
/// * **`send($tx:expr, eval $expr:expr) => ...`:** Attempt to `send` the result of
///   evaluating `$expr` with `$tx`. *Warning:* $expr is evaluated once in every
///   loop iteration. In some iterations, its result may simply be discarded
///   with no actual attempt at sending it.
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
        #[allow(unused_mut, unused_variables)]
        {
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
            #[allow(unreachable_code)]
            'select: loop {
                // This double-loop construct is used to guard against the user
                // using unlabeled breaks / continues in their code.
                //
                // It works by abusing rustc's control flow analysis:
                //
                // 1) The _guard variable is declared
                // 2) The generated code will assign a value to _guard [A0]
                //    immediately after an operation was successful (before
                //    running any user code)
                // 3) There is another assignment to _guard [A1] directly after
                //    the inner loop.
                // 4) If the user has an unlabeled break in their code, rustc
                //    will complain because both [A0] and [A1] assign to guard.
                //    If the user has an unlabeled continue in their code, rustc
                //    will complain because [A0] may assign to guard twice.
                // 5) Directly after executing the user code, we break the
                //    outer loop, so we don't trigger the error ourselves.
                // 6) To avoid an infinite loop, we manually continue to the
                //    outer loop at the very end of the inner loop.
                let _dont_use_an_unlabeled_continue_or_break_in_select_loop;
                loop {
                    $(
                        // Build the actual method invocations.
                        select_loop! {
                            @impl(
                                'select,
                                state,
                                _dont_use_an_unlabeled_continue_or_break_in_select_loop
                            )
                            $method($($args)*) => $body
                        }
                    )*
                    continue 'select;
                }
                _dont_use_an_unlabeled_continue_or_break_in_select_loop = ();
            }
        }
    };

    //The individual method invocations
    {@impl($select:tt, $state:ident, $guard:ident) send($tx:expr, $val:ident) => $body:expr} => {
        match $state.send(&*&$tx, $val) {
            Ok(()) => {
                $guard = ();
                break $select $body;
            }
            Err($crate::SelectSendError(val)) => {
                $val = val;
            }
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) send($tx:expr, mut $val:expr) => $body:expr} => {
        match $state.send(&*&$tx, $val) {
            Ok(()) => {
                $guard = ();
                break $select $body;
            }
            Err($crate::SelectSendError(val)) => {
                $val = val;
            }
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) send($tx:expr, eval $val:expr) => $body:expr} => {
        if let Ok(()) = $state.send(&*&$tx, $val) {
            $guard = ();
            break $select $body;
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) recv($rx:expr, _) => $body:expr} => {
        if let Ok(_) = $state.recv(&*&$rx) {
            $guard = ();
            break $select $body;
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) recv($rx:expr, $val:ident) => $body:expr} => {
        if let Ok($val) = $state.recv(&*&$rx) {
            $guard = ();
            break $select $body;
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) recv($rx:expr, mut $val:ident) => $body:expr} => {
        if let Ok(mut $val) = $state.recv(&*&$rx) {
            $guard = ();
            break $select $body;
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) disconnected() => $body:expr} => {
        if $state.disconnected() {
            $guard = ();
            break $select $body;
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) would_block() => $body:expr} => {
        if $state.would_block() {
            $guard = ();
            break $select $body;
        }
    };
    {@impl($select:tt, $state:ident, $guard:ident) timed_out($_timeout:expr) => $body:expr} => {
        if $state.timed_out() {
            $guard = ();
            break $select $body;
        }
    };

    // The prelude helpers
    {@prelude($state:ident) send($tx:expr, $val:ident)} => {
        let mut $val = $val;
    };
    {@prelude($state:ident) timed_out($timeout:expr)} => {
        let mut $state = $crate::Select::with_timeout($timeout);
    };
    {@prelude($state:ident) $($tail:tt)*} => {};
}

#[cfg(test)]
mod tests {
    use ::{Receiver, Sender};
    use std::time::Duration;

    struct _Foo(String);

    fn _it_compiles(
        mut struct_val: _Foo,
        immutable_var: String,
        eval_var: String,
        rx0: Receiver<String>,
        rx1: &Receiver<u32>,
        rx2: Receiver<String>,
        rx3: Receiver<String>,
        tx0: &mut Sender<String>,
        tx1: Sender<String>,
        tx2: Sender<String>,
        tx3: Sender<String>,
        tx4: Sender<String>,
        tx5: Sender<u32>,
    ) -> Option<String> {
        select_loop! {
            recv(rx0, val) => Some(val),
            recv(rx1, val) => Some(val.to_string()),
            recv(rx2, mut val) => Some(val.as_mut_str().to_owned()),
            recv(rx3, _) => None,
            send(tx0, mut struct_val.0) => Some(immutable_var),
            send(tx1, immutable_var) => Some(struct_val.0),
            send(tx2, eval struct_val.0.clone()) => Some(struct_val.0),
            send(tx3, immutable_var) => Some(eval_var),
            send(tx4, eval eval_var.clone()) => Some(eval_var),
            send(tx5, eval 42) => None,
            disconnected() => Some("disconnected".into()),
            would_block() => Some("would_block".into()),
            timed_out(Duration::from_secs(1)) => Some("timed_out".into()),
            //The previous timeout duration is overwritten
            timed_out(Duration::from_secs(2)) => Some("timet_out".into()),
        }
    }
}
