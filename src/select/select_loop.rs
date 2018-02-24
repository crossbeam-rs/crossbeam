/// The static selection macro.
///
/// It allows declaring an arbitrary static list of operations on channels, and waiting until
/// exactly one of them fires. If you need to select over a dynamic list of operations, use
/// [`Select`] instead. This macro is just a restricted and more user-friendly wrapper around it.
///
/// # What is selection?
///
/// It is possible to declare a set of possible send and/or receive operations on channels, and
/// then wait until exactly one of them fires (in other words, one of them is *selected*).
///
/// For example, we might want to receive a message from a set of two channels and block until a
/// message is received from any of them. To do that, we would write:
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// use std::thread;
///
/// let (tx1, rx1) = channel::unbounded();
/// let (tx2, rx2) = channel::unbounded();
///
/// thread::spawn(move || tx1.send("foo").unwrap());
/// thread::spawn(move || tx2.send("bar").unwrap());
///
/// select_loop! {
///     recv(rx1, msg) => println!("A message was received from rx1: {:?}", msg),
///     recv(rx2, msg) => println!("A message was received from rx2: {:?}", msg),
/// }
///
/// # }
/// ```
///
/// There are two selection *cases*: a receive on `rx1` and a receive on `rx2`. The macro is in
/// fact loop, which is continuously probing both channels until one of the cases successfully
/// receives a message. Then we print the message and the loop is broken.
///
/// The loop will automatically block the current thread if none of the operations can proceed and
/// wake up as soon as any one of them becomes ready. However, there are a few rules that must be
/// followed when declaring a set of cases in a loop.
///
/// # Selection cases
///
/// There are five kinds of selection cases:
///
/// 1. A *receive* case, which fires when a message can be received from the channel.
/// 2. A *send* case, which fires when the message can be sent into the channel.
/// 3. A *would block* case, which fires when all receive and send operations in the loop would
///    block.
/// 4. A *closed* case, which fires when all operations in the loop are working with closed
///    channels.
/// 5. A *timed out* case, which fires when selection is blocked for longer than the specified
///    timeout.
///
/// Additionally, every case may optionally be guarded by a condition, so that the case is enabled
/// only if the condition is true. Note that such conditions must not change within the
/// `select_loop!`.
///
/// # Selection rules
///
/// Rules which must be respected in order for selection to work properly:
///
/// 1. No selection case may be repeated.
/// 2. No two cases may operate on the same end (receiving or sending) of the same channel.
/// 3. There must be at least one *send* or at least one *recv* case.
/// 4. If case conditions are used, they must not change within the `select_loop!`.
///
/// Violating any of these rules will either result in a panic, deadlock, or livelock, possibly
/// even in a seemingly unrelated send or receive operations outside this particular selection
/// loop.
///
/// # Guarantees
///
/// 1. Exactly one case fires.
/// 2. If none of the cases can fire at the time, the current thread will be blocked.
/// 3. If blocked, the current thread will be woken up as soon a message is pushed/popped into/from
///    any channel waited on by a receive/send case, or if all channels get closed.
///
/// Finally, if more than one send or receive case can fire at the same time, a pseudorandom case
/// will be selected, but on a best-effort basis only. The mechanism isn't promising any strict
/// guarantees on fairness.
///
/// # Syntax
///
/// The macro has similar syntax to `match` expression. It takes a list of cases, where each case
/// is of the form `operation(arguments) => expression`. The individual cases are optionally
/// separated by commas. Just like `match`, the whole macro invocation is an expression, which in
/// the end evaluates to a single value.
///
/// Every case may also optionally have a guard expression at the end, in the form of
/// `operation(arguments) if guard_expression => expression`. The guarded case will participate in
/// selection only if `guard_expression` evaluates to `true`.
///
/// The following code illustrates the various ways in which cases can be declared:
///
/// ```ignore
/// select_loop! {
///     // Send `msg` into `tx1`.
///     //
///     // Behind the scenes, this form will actually rebind the variable in mutable form by
///     // inserting the following line before the loop: `let mut msg = msg;`
///     send(tx1, msg) => { ... }
///
///     // Send the result of an expression as a message into `tx2`.
///     //
///     // Note that this form will evaluate the expression in each iteration of the loop. If the
///     // evaluation is expensive, you should probably do it once before the loop and bind to a
///     // variable, then send that variable as the message.
///     send(tx2, x * 10 - y) => { ... }
///
///     // Send `msg` into `tx3`, but regain ownership on each failure.
///     //
///     // If sending the message fails in an interation of the loop, then ownership of the message
///     // will be automatically regained from the error and bound back to the original variable.
///     //
///     // You should use this form if `msg` is not `Copy`.
///     send(tx3, mut msg) => { ... }
///
///     // Send `msg` into `tx4` but only if `enabled_tx4` is `true`.
///     send(tx4, msg) if enabled_tx4 => { ... }
///
///     // Receive `msg` from `rx1`.
///     recv(rx1, msg) => { ... }
///
///     // Receive `msg` from `rx2`, and make the variable mutable.
///     recv(rx2, mut msg) => { ... }
///
///     // Receive a message from `rx3`, but don't bind it to a variable.
///     recv(rx3, _) => { ... }
///
///     // Receive `msg` from the optional receiver, if it exists (`Option<Receiver<_>>`).
///     recv(opt_rx4.as_ref().unwrap(), msg) if opt_rx4.is_some() => { ... }
///
///     // This case fires if all declared send/receive operations are on closed channels.
///     closed() => { ... }
///
///     // This case fires if all declared send/receive operations would block.
///     would_block() => { ... }
///
///     // This case fires if selection waits for longer than `timeout`.
///     //
///     // The specified `timeout` must be of type `std::time::Duration`.
///     timed_out(timeout) => { ... }
/// }
/// ```
///
/// # Examples
///
/// ## Receive a message of the same type from two channels
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// use std::thread;
///
/// let (tx1, rx1) = channel::unbounded();
/// let (tx2, rx2) = channel::unbounded();
///
/// thread::spawn(move || tx1.send("foo").unwrap());
/// thread::spawn(move || tx2.send("bar").unwrap());
///
/// let msg = select_loop! {
///     recv(rx1, msg) => {
///         println!("Received from rx1.");
///         msg
///     }
///     recv(rx2, msg) => {
///         println!("Received from rx2.");
///         msg
///     }
/// };
///
/// println!("Message: {:?}", msg);
///
/// # }
/// ```
///
/// ## Send a non-`Copy` message, regaining ownership on each failure
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// let (tx, rx) = channel::unbounded();
///
/// // The message we're going to send.
/// let mut msg = "Hello!".to_string();
///
/// select_loop! {
///     // The variable is marked with `mut`, which indicates that ownership must be reacquired if
///     // sending the variable fails.
///     send(tx, mut msg) => println!("The message was sent!"),
///
///     recv(rx, msg) => println!("A message was received: {}", msg),
/// }
///
/// # }
/// ```
///
/// ## Stop if all channels are closed
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// let (tx, rx) = channel::unbounded();
///
/// // Close the channel.
/// drop(rx);
///
/// select_loop! {
///     // Won't happen. The channel is closed.
///     send(tx, "message") => {
///         println!("Sent the message.");
///         panic!();
///     }
///
///     closed() => println!("All channels are closed! Stopping selection."),
/// }
///
/// # }
/// ```
///
/// ## Stop if all operations would block
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// let (tx, rx) = channel::unbounded::<i32>();
///
/// select_loop! {
///     // Won't happen. The channel is empty.
///     recv(rx, msg) => {
///         println!("Received message: {:?}", msg);
///         panic!();
///     }
///
///     would_block() => println!("All operations would block. Stopping selection."),
/// }
///
/// # }
/// ```
///
/// ## Selection with a timeout
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// use std::time::Duration;
///
/// let (tx, rx) = channel::unbounded::<i32>();
///
/// select_loop! {
///     // Won't happen. The channel is empty.
///     recv(rx, msg) => {
///         println!("Received message: {:?}", msg);
///         panic!();
///     }
///
///     timed_out(Duration::from_secs(1)) => println!("Timed out after 1 second."),
/// }
///
/// # }
/// ```
///
/// ## One send and one receive operation on the same channel
///
/// ```
/// # #[macro_use]
/// # extern crate crossbeam_channel as channel;
/// # fn main() {
///
/// use channel::{Sender, Receiver, Select};
/// use std::thread;
///
/// // Either send my name into the channel or receive someone else's, whatever happens first.
/// fn seek<'a>(name: &'a str, tx: Sender<&'a str>, rx: Receiver<&'a str>) {
///     select_loop! {
///         recv(rx, peer) => println!("{} received a message from {}.", name, peer),
///         send(tx, name) => {}
///     }
/// }
///
/// let (tx, rx) = channel::bounded(1); // Make room for one unmatched send.
///
/// // Pair up five people by exchanging messages over the channel.
/// // Since there is an odd number of them, one person won't have its match.
/// ["Anna", "Bob", "Cody", "Dave", "Eva"].iter()
///     .map(|name| {
///         let tx = tx.clone();
///         let rx = rx.clone();
///         thread::spawn(move || seek(name, tx, rx))
///     })
///     .collect::<Vec<_>>()
///     .into_iter()
///     .for_each(|t| t.join().unwrap());
///
/// // Let's send a message to the remaining person who doesn't have a match.
/// if let Ok(name) = rx.try_recv() {
///     println!("No one received {}’s message.", name);
/// }
///
/// # }
/// ```
///
/// [`Select`]: struct.Select.html
#[macro_export]
macro_rules! select_loop {
    // The main entry point.
    {$($method:ident($($args:tt)*) $(if $guard:expr)* => $body:expr$(,)*)*} => {{
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
            $(select_loop!(@prelude(state) $method($($args)*) $(if $guard)*);)*

            // Only allow one repetition of the `$guard`.
            $(select_loop!(@check_guard($(if $guard)*) [$method($($args)*) $(if $guard)* =>]);)*

            // The actual select loop which a user would write manually
            loop {
                #[allow(bad_style)]
                struct _DONT_USE_AN_UNLABELED_BREAK_IN_SELECT_LOOP;

                $(
                    // Only enable this case if `$guard` is true.
                    if $($guard &&)* true {
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
                    }
                )*
            }
        }
    }};

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
    {@impl($state:ident) recv($rx:expr, $val:pat) => $body:expr} => {
        if let Ok(val) = $state.recv(&*&$rx) {
            let $val = val;  // Only allow infallible patterns
            $body
        }
    };
    {@impl($state:ident) closed() => $body:expr} => {
        if $state.closed() {
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
    {@prelude($state:ident) send($tx:expr, $val:ident) $(if $_guard:expr)*} => {
        #[allow(unused_mut, unused_variables)]
        let mut $val = $val;
    };
    {@prelude($state:ident) timed_out($timeout:expr) $(if $guard:expr)*} => {
        if $($guard &&)* true {
            $state = $crate::Select::with_timeout($timeout);
        }
    };
    {@prelude($state:ident) $($tail:tt)*} => {};

    // Additional syntax validation for the guard expression
    {@check_guard() [$($_ctx:tt)*]} => {};
    {@check_guard(if $_guard:expr) [$($_ctx:tt)*]} => {};
    {@check_guard($($_tt:tt)*) [$($ctx:tt)*]} => {
        compile_error!(
            concat!(
                "multiple guards were supplied to `select_loop!`: `",
                stringify!($($ctx)*),
                "`"));
    }
}
