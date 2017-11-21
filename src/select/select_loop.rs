#[macro_export]
macro_rules! select_loop {
    {$($method:ident($($args:tt)*) => $body:expr$(,)*)*} => {
        #[allow(unused_mut)]
        {
            $(select_loop!(@make_mut $method($($args)*));)*

            let mut state = $crate::Select::new();

            #[allow(unreachable_code)]
            'select: loop {
                let _dont_use_an_unlabeled_continue_or_break_in_select_loop;
                loop {
                    $(
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
    {@impl($select:tt, $state:ident, $guard:ident) recv($rx:expr, $val:ident) => $body:expr} => {
        if let Ok($val) = $state.recv(&*&$rx) {
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
    {@impl($select:tt, $state:ident, $guard:ident) timed_out() => $body:expr} => {
        if $state.timed_out() {
            $guard = ();
            break $select $body;
        }
    };

    {@make_mut send($tx:expr, $val:ident)} => {
        let mut $val = $val;
    };
    {@make_mut $($tail:tt)*} => {};
}

#[cfg(test)]
mod tests {
    use ::{Receiver, Sender};

    struct _Foo(String);

    fn _it_compiles(
        mut struct_val: _Foo,
        immutable_var: String,
        eval_var: String,
        rx0: Receiver<String>,
        rx1: &Receiver<u32>,
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
            send(tx0, mut struct_val.0) => Some(immutable_var),
            send(tx1, immutable_var) => Some(struct_val.0),
            send(tx2, eval struct_val.0.clone()) => Some(struct_val.0),
            send(tx3, immutable_var) => Some(eval_var),
            send(tx4, eval eval_var.clone()) => Some(eval_var),
            send(tx5, eval 42) => None,
            disconnected() => Some("disconnected".into()),
            would_block() => Some("would_block".into()),
            timed_out() => Some("timed_out".into()),
        }
    }
}
