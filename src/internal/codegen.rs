/// Code generator for the `select!` macro.

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
            #[allow(unused_imports)]
            use $crate::internal::select::RecvArgument;
            #[allow(unused_imports)]
            use $crate::internal::channel::Receiver;

            // TODO: document that we can't expect a mut iterator here because of Clone
            match &mut (&$rs).__as_recv_argument() {
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
            #[allow(unused_imports)]
            use $crate::internal::select::SendArgument;
            #[allow(unused_imports)]
            use $crate::internal::channel::Sender;

            match &mut (&$ss).__as_send_argument() {
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
    ) => {
        __crossbeam_channel_codegen!(@mainloop $recv $send $default)
    };

    (@mainloop $recv:tt $send:tt $default:tt) => {{
        use std::time::Instant;

        use $crate::internal::select::CaseId;
        use $crate::internal::select::Token;
        use $crate::internal::context;
        use $crate::internal::utils;

        #[allow(unused_imports)]
        use $crate::internal::select::Select;

        let deadline: Option<Instant>;
        let default_index: usize;
        __crossbeam_channel_codegen!(@default deadline default_index $default);

        let mut cases;
        __crossbeam_channel_codegen!(@container cases $recv $send);

        let mut token: Token = unsafe { ::std::mem::uninitialized() };
        let mut index: usize = !0;
        let mut selected: usize = 0;

        if cases.len() >= 2 {
            utils::shuffle(&mut cases);
        }

        loop {
            for &(sel, i, addr) in &cases {
                if sel.try(&mut token) {
                    index = i;
                    selected = addr;
                    break;
                }
            }

            if index != !0 {
                break;
            }

            for &(sel, i, addr) in &cases {
                if sel.retry(&mut token) {
                    index = i;
                    selected = addr;
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

            context::current_reset();

            // TODO: fairness test with three channel types: unbounded, bounded(1), bounded(0)
            // TODO: change fairness tests: track counters and verify hits/rounds >= X%

            for case in &cases {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, _, _) = case;

                if !sel.promise(&mut token, case_id) {
                    context::current_try_abort();
                    break;
                }
                if context::current_selected() != CaseId::none() {
                    break;
                }
            }

            let timed_out = !context::current_wait_until(deadline);
            let s = context::current_selected();

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
                        if sel.fulfill(&mut token) {
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

            if cases.len() >= 2 {
                utils::shuffle(&mut cases);
            }
        }

        // This drop is just to ignore a warning complaining about unused `selected`.
        drop(selected);

        __crossbeam_channel_codegen!(@finish token index selected $recv $send $default)

        // TODO: Run `cargo clippy` and make sure there are no warnings in here.
        // TODO: Add a Travis test for clippy
    }};

    (@container
        $cases:ident
        (($i:tt $var:ident) recv($rs:expr, $m:pat, $r:pat) => $body:tt,)
        ()
    ) => {
        use $crate::internal::smallvec::SmallVec;
        let mut c: SmallVec<[_; 4]> = SmallVec::new();
        while let Some(r) = $var.next() {
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
        use $crate::internal::smallvec::SmallVec;
        let mut c: SmallVec<[_; 4]> = SmallVec::new();
        while let Some(s) = $var.next() {
            let addr = s as *const Sender<_> as usize;
            c.push((s, $i, addr));
        }
        $cases = c;
    };
    (@container
        $cases:ident
        $recv:tt
        $send:tt
    ) => {
        use $crate::internal::smallvec::SmallVec;
        $cases = SmallVec::<[(&Select, usize, usize); 4]>::new();
        __crossbeam_channel_codegen!(@push $cases $recv $send);
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
        __crossbeam_channel_codegen!(@push $cases ($($tail)*) $send);
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
        __crossbeam_channel_codegen!(@push $cases () ($($tail)*));
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
        if let Some(instant) = $crate::internal::select::DefaultArgument::default_argument($t) {
            $deadline = Some(instant);
            $default_index = $i;
        } else {
            $deadline = None;
            $default_index = !0;
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
            let ($m, $r) = unsafe {
                let r = bind(&$var, $selected);
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

                // We have to prefix variables with an underscore to get rid of warnings in
                // case `$m` is of type `!`.
                let _s = unsafe { bind(&$var, $selected) };

                let _guard = Guard(|| {
                    eprintln!(
                        "a send case triggered a panic while evaluating the message, {}:{}:{}",
                        file!(),
                        line!(),
                        column!(),
                    );
                    ::std::process::abort();
                });

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
        (($i:tt $var:ident) default $args:tt => $body:tt,)
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
        unreachable!()
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
