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
            use $crate::select::RecvArgument;
            #[allow(unused_imports)]
            use $crate::channel::Receiver;

            match &mut (&$rs).to_receivers() {
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
            use $crate::select::SendArgument;
            #[allow(unused_imports)]
            use $crate::channel::Sender;

            match &mut (&$ss).to_senders() {
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

        // These cause warnings:
        use $crate::select::Select;

        use $crate::select::CaseId;
        use $crate::select::Token;
        use $crate::context;
        use $crate::utils::{Backoff, shuffle};

        let deadline: Option<Instant>;
        let default_index: usize;
        __crossbeam_channel_codegen!(@default deadline default_index $default);

        let mut cases;
        __crossbeam_channel_codegen!(@container cases $recv $send);

        let mut token: Token = unsafe { ::std::mem::zeroed() };
        let mut index: usize = !0;
        let mut selected: usize = 0;

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

                // TODO: break here? (should speed up zero-capacity channels!)
                // break;
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

            for case in &cases {
                let case_id = CaseId::new(case as *const _ as usize);
                let &(sel, _, _) = case;
                sel.promise(&mut token, case_id);
            }

            for &(sel, _, _) in &cases {
                if !sel.is_blocked() {
                    context::current_try_select(CaseId::abort());
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

        // This drop is just to ignore a warning complaining about unused `selected`.
        drop(selected);

        __crossbeam_channel_codegen!(@finish token index selected $recv $send $default)

        // TODO: optimize send, try_recv, recv
        // TODO: need a select mpmc test for all flavors

        // TODO: test select with duplicate cases - and make sure all of them fire (fairness)!

        // TODO: test sending and receiving into the same channel from the same thread (all flavors)

        // TODO: allocate less memory in unbounded flavor if few elements are sent.
        // TODO: allocate memory lazily in unbounded flavor?
        // TODO: Run `cargo clippy` and make sure there are no warnings in here.

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
        use $crate::smallvec::SmallVec;
        $cases = SmallVec::<[(&Select, usize, usize); 4]>::new();
        __crossbeam_channel_codegen!(@push $cases $recv $send);
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
        __crossbeam_channel_codegen!(@push $cases ($($tail)*) $send);
    };
    (@push
        $cases:ident
        ()
        (($i:tt $var:ident) send($ss:expr, $m:expr, $s:pat) => $body:tt, $($tail:tt)*)
    ) => {
        for s in $var.clone() {
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
        if let Some(instant) = $crate::select::DefaultArgument::to_instant($t) {
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
                let msg = r.read(&mut $token);
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

                let s = unsafe { bind(&$var, $selected) };

                let msg = {
                    // We have to prefix variables with an underscore to get rid of warnings in
                    // case `$m` is of type `!`.
                    let guard = Guard(|| {
                        eprintln!(
                            "a send case triggered a panic while evaluating the message, {}:{}:{}",
                            file!(),
                            line!(),
                            column!(),
                        );
                        ::std::process::abort();
                    });

                    let msg = $m;
                    ::std::mem::forget(guard);
                    msg
                };

                unsafe { s.write(&mut $token, msg); }
                s
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

    (@$($tokens:tt)*) => {
        compile_error!(concat!(
            "internal error in crossbeam-channel: ",
            stringify!(@$($tokens)*),
        ));
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
