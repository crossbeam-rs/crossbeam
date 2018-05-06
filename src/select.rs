use smallvec::SmallVec;
use channel::{Token, Receiver, Sender};
use utils::Backoff;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CaseId {
    pub id: usize,
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
        __crossbeam_channel_parse!(
            __crossbeam_channel_generate
            $($case $(($($args)*))* => $body,)*
        )
    };
    ($($tokens:tt)*) => {
        __crossbeam_channel_parse!(
            __crossbeam_channel_generate
            $($tokens)*
        )
    };
}

pub trait Sel {
    type Token;
    fn try(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool;
    fn promise(&self, token: &mut Self::Token, case_id: CaseId);
    fn is_blocked(&self) -> bool;
    fn revoke(&self, case_id: CaseId);
    fn fulfill(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool;
}

impl<'a, T: Sel> Sel for &'a T {
    type Token = <T as Sel>::Token;
    fn try(&self, token: &mut Self::Token, backoff: &mut Backoff) -> bool {
        (**self).try(token, backoff)
    }
    fn promise(&self, token: &mut Self::Token, case_id: CaseId) {
        (**self).promise(token, case_id);
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

pub trait SendArgument<'a, T: 'a> {
    type Iter: Iterator<Item = &'a Sender<T>>;
    fn to_senders(&'a self) -> Self::Iter;
}

impl<'a, T> SendArgument<'a, T> for &'a Sender<T> {
    type Iter = ::std::option::IntoIter<&'a Sender<T>>;
    fn to_senders(&'a self) -> Self::Iter {
        Some(*self).into_iter()
    }
}

impl<'a, T: 'a, I: IntoIterator<Item = &'a Sender<T>> + Clone> SendArgument<'a, T> for I {
    type Iter = <I as IntoIterator>::IntoIter;
    fn to_senders(&'a self) -> Self::Iter {
        self.clone().into_iter()
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! __crossbeam_channel_generate {
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
                    __crossbeam_channel_generate!(
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
                    __crossbeam_channel_generate!(
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
        __crossbeam_channel_generate!(@mainloop $recv $send $default)
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
        use $crate::context::{self, Context};
        use $crate::utils::{Backoff, shuffle};

        let deadline: Option<Instant>;
        let default_index: usize;
        __crossbeam_channel_generate!(@default deadline default_index $default);

        let mut cases;
        __crossbeam_channel_generate!(@container cases $recv $send);

        let mut token: Token = unsafe { ::std::mem::zeroed() };
        let mut index: usize = !0;
        let mut selected: usize = 0;

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

        __crossbeam_channel_generate!(@finish token index selected $recv $send $default)

        // TODO: optimize send, try_recv, recv
        // TODO: allow recv(r) case

        // TODO: need a select mpmc test for all flavors

        // TODO: test select with duplicate cases - and make sure all of them fire (fairness)!

        // TODO: accept both Instant and Duration in the default case

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
        $cases = SmallVec::<[(&Sel<Token = Token>, usize, usize); 4]>::new();
        __crossbeam_channel_generate!(@push $cases $recv $send);
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
        __crossbeam_channel_generate!(@push $cases ($($tail)*) $send);
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
        __crossbeam_channel_generate!(@push $cases () ($($tail)*));
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
                (msg, r)
            };
            $body
        } else {
            __crossbeam_channel_generate!(
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
                    let _guard = Guard(|| {
                        // TODO: report panic with eprintln
                        ::std::process::abort();
                    });
                    let _msg = $m;

                    #[allow(unreachable_code)]
                    {
                        ::std::mem::forget(_guard);
                        _msg
                    }
                };

                s.write(&mut $token, _msg);
                s
            };
            $body
        } else {
            __crossbeam_channel_generate!(
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
            __crossbeam_channel_generate!(
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
        compile_error!(concat!("internal error in crossbeam-channel: ", stringify!(@$($tokens)*)));
    };

    // The entry point.
    (($($recv:tt)*) ($($send:tt)*) $default:tt) => {
        __crossbeam_channel_generate!(
            @declare
            ($($recv)* $($send)*)
            ($($recv)*)
            ($($send)*)
            $default
        )
    }
}
