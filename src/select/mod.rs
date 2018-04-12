use std::fmt;
use std::time::{Duration, Instant};

use {Receiver, Sender};
// use err::{SelectRecvError, SelectSendError};
use self::machine::Machine;

pub use self::case_id::CaseId;

mod case_id;
mod machine;
mod select_macro;
// mod select_loop;

#[doc(hidden)]
pub mod handle;

/*
/// The dynamic selection interface.
///
/// It allows declaring an arbitrary (possibly dynamic) list of operations on channels, and waiting
/// until exactly one of them fires. The interface is somewhat tricky to use and care must be taken
/// in order to use it correctly.
///
/// If possible, it is highly recommended to use the [`select_loop!`] macro instead, which is much
/// easier to use. The downside of the macro is that it only allows selecting over a statically
/// defined set of operations.
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
/// use std::thread;
/// use crossbeam_channel::{unbounded, Select};
///
/// let (tx1, rx1) = unbounded();
/// let (tx2, rx2) = unbounded();
///
/// thread::spawn(move || tx1.send("foo").unwrap());
/// thread::spawn(move || tx2.send("bar").unwrap());
///
/// let mut sel = Select::new();
/// loop {
///     if let Ok(msg) = sel.recv(&rx1) {
///         println!("A message was received from rx1: {:?}", msg);
///         break;
///     }
///     if let Ok(msg) = sel.recv(&rx2) {
///         println!("A message was received from rx2: {:?}", msg);
///         break;
///     }
/// }
/// ```
///
/// There are two selection *cases*: a receive on `rx1` and a receive on `rx2`. The loop is
/// continuously probing both channels until one of the cases successfully receives a message. Then
/// we print the message and the loop is broken.
///
/// Note that `sel` holds an internal state machine that keeps track of how many cases there are,
/// which channels are closed, etc. It is smart enough to automatically register each case into an
/// internal conditional variable of sorts, block on it, and wake up when any of the cases become
/// ready.
///
/// You don't need to wory about blocking or about the loop burning CPU time - the selection
/// mechanism will automatically block and wake up the current thread as is necessary. However,
/// there are a few rules that must be followed when probing cases in a loop.
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
/// # Selection rules
///
/// Rules which must be respected in order for selection to work properly:
///
/// 1. Before each selection, a fresh [`Select`] must be created.
/// 2. Selection cases must be repeatedly probed in a loop.
/// 3. If a selection case fires, the loop must be broken without probing cases any further.
/// 4. In each iteration of the loop, the same set of cases must be probed in the same order.
/// 5. No selection case may be repeated.
/// 6. No two cases may operate on the same end (receiving or sending) of the same channel.
/// 7. There must be at least one *send*, or at least one *recv* case.
///
/// Violating any of these rules will either result in a panic, deadlock, or livelock, possibly
/// even in a seemingly unrelated send or receive operations outside this particular selection
/// loop.
///
/// # Guarantees
///
/// 1. Exactly one case fires.
/// 2. If none of the cases can fire at the time, one of the calls in the loop will block the
///    current thread.
/// 3. If blocked, the current thread will be woken up as soon a message is pushed/popped into/from
///    any channel waited on by a receive/send case, or if all channels get closed.
///
/// Finally, if more than one send or receive case can fire at the same time, a pseudorandom case
/// will be selected, but on a best-effort basis only. The mechanism isn't promising any strict
/// guarantees on fairness.
///
/// # Examples
///
/// ## Receive a message of the same type from two channels
///
/// ```
/// use std::thread;
/// use crossbeam_channel::{unbounded, Select};
///
/// let (tx1, rx1) = unbounded();
/// let (tx2, rx2) = unbounded();
///
/// thread::spawn(move || tx1.send("foo").unwrap());
/// thread::spawn(move || tx2.send("bar").unwrap());
///
/// let mut sel = Select::new();
/// let msg = loop {
///     if let Ok(msg) = sel.recv(&rx1) {
///         println!("Received from rx1.");
///         break msg;
///     }
///     if let Ok(msg) = sel.recv(&rx2) {
///         println!("Received from rx2.");
///         break msg;
///     }
/// };
///
/// println!("Message: {:?}", msg);
/// ```
///
/// ## Send a non-`Copy` message, regaining ownership on each failure
///
/// ```
/// use crossbeam_channel::{unbounded, Select};
///
/// let (tx, rx) = unbounded();
///
/// // The message we're going to send.
/// let mut msg = "Hello!".to_string();
///
/// let mut sel = Select::new();
/// loop {
///     if let Err(err) = sel.send(&tx, msg) {
///         // This selection case didn't fire yet.
///         // Regain ownership of the message, which is contained in `err`.
///         msg = err.0;
///     } else {
///         // The message was successfully sent.
///         break;
///     }
/// }
/// ```
///
/// ## Stop if all channels are closed
///
/// ```
/// /*
/// use crossbeam_channel::{unbounded, Select};
///
/// let (tx, rx) = unbounded();
///
/// // Close the channel.
/// drop(rx);
///
/// let mut sel = Select::new();
/// loop {
///     if let Ok(_) = sel.send(&tx, "message") {
///         // Won't happen. The channel is closed.
///         println!("Sent the message.");
///         panic!();
///         break;
///     }
///     if sel.closed() {
///         println!("All channels are closed! Stopping selection.");
///         break;
///     }
/// }
/// */
/// ```
///
/// ## Stop if all operations would block
///
/// ```
/// use crossbeam_channel::{unbounded, Select};
///
/// let (tx, rx) = unbounded::<i32>();
///
/// let mut sel = Select::new();
/// loop {
///     if let Ok(msg) = sel.recv(&rx) {
///         // Won't happen. The channel is empty.
///         println!("Received message: {:?}", msg);
///         panic!();
///         break;
///     }
///     if sel.would_block() {
///         println!("All operations would block. Stopping selection.");
///         break;
///     }
/// }
/// ```
///
/// ## Selection with a timeout
///
/// ```
/// use std::time::Duration;
/// use crossbeam_channel::{unbounded, Select};
///
/// let (tx, rx) = unbounded::<i32>();
///
/// let mut sel = Select::with_timeout(Duration::from_secs(1));
/// loop {
///     if let Ok(msg) = sel.recv(&rx) {
///         // Won't happen. The channel is empty.
///         println!("Received message: {:?}", msg);
///         panic!();
///         break;
///     }
///     if sel.timed_out() {
///         println!("Timed out after 1 second.");
///         break;
///     }
/// }
/// ```
///
/// ## One send and one receive operation on the same channel
///
/// ```
/// use crossbeam_channel::{bounded, Sender, Receiver, Select};
/// use std::thread;
///
/// // Either send my name into the channel or receive someone else's, whatever happens first.
/// fn seek<'a>(name: &'a str, tx: Sender<&'a str>, rx: Receiver<&'a str>) {
///     let mut sel = Select::new();
///     loop {
///         if let Ok(peer) = sel.recv(&rx) {
///             println!("{} received a message from {}.", name, peer);
///             break;
///         }
///         if let Ok(()) = sel.send(&tx, name) {
///             // Wait for someone to receive my message.
///             break;
///         }
///     }
/// }
///
/// let (tx, rx) = bounded(1); // Make room for one unmatched send.
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
/// ```
///
/// ## Receive a message from a dynamic list of receivers
///
/// ```
/// use std::thread;
/// use crossbeam_channel::{unbounded, Select};
///
/// let mut chans = vec![];
/// for _ in 0..10 {
///     chans.push(unbounded());
/// }
///
/// let tx = chans[7].0.clone();
///
/// thread::spawn(move || {
///     let mut sel = Select::new();
///
///     let msg = 'select: loop {
///         // In each iteration of the selection loop we probe cases in the same order.
///         for &(_, ref rx) in &chans {
///             if let Ok(msg) = sel.recv(rx) {
///                 // Finally, this case fired.
///                 // Break the outer loop with the received message as the result.
///                 break 'select msg;
///             }
///         }
///     };
///
///     println!("Received message: {:?}", msg);
/// });
///
/// tx.send("Hello!").unwrap();
/// ```
///
/// [`Select`]: struct.Select.html
/// [`select_loop!`]: macro.select_loop.html
pub struct Select {
    machine: Machine,
}

impl Select {
    /// Constructs a new state machine for selection.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let mut sel = Select::new();
    /// ```
    #[inline]
    pub fn new() -> Select {
        Select {
            machine: Machine::new(),
        }
    }

    /// Constructs a new state machine for selection with a specific `timeout`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let mut sel = Select::with_timeout(Duration::from_secs(5));
    /// ```
    #[inline]
    pub fn with_timeout(timeout: Duration) -> Select {
        Select {
            machine: Machine::with_deadline(Some(Instant::now() + timeout)),
        }
    }

    /// Probes a *send* case.
    ///
    /// The operation is attempting to send `msg` through `tx`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (tx, rx) = unbounded();
    ///
    /// let mut sel = Select::new();
    /// loop {
    ///     if let Ok(_) = sel.send(&tx, "foo") {
    ///         // The message was successfully sent.
    ///         break;
    ///     }
    /// }
    /// ```
    pub fn send<T>(&mut self, tx: &Sender<T>, msg: T) -> Result<(), SelectSendError<T>> {
        self.machine.send(tx, msg).map_err(SelectSendError)
    }

    /// Probes a *receive* case.
    ///
    /// The operation is attempting to receive a message through `rx`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (tx, rx) = unbounded();
    /// tx.send("foo").unwrap();
    ///
    /// let mut sel = Select::new();
    /// loop {
    ///     if let Ok(msg) = sel.recv(&rx) {
    ///         // The message was successfully received.
    ///         assert_eq!(msg, "foo");
    ///         break;
    ///     }
    /// }
    /// ```
    pub fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, SelectRecvError> {
        self.machine.recv(rx).map_err(|_| SelectRecvError)
    }

    /// Probes a *closed* case.
    ///
    /// This case fires when all operations in the loop are working with closed channels.
    ///
    /// # Examples
    ///
    /// ```
    /// /*
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (tx, rx) = unbounded();
    ///
    /// // Close the channel.
    /// drop(rx);
    ///
    /// let mut sel = Select::new();
    /// loop {
    ///     if let Ok(_) = sel.send(&tx, "foo") {
    ///         // The message was successfully sent.
    ///         panic!();
    ///         break;
    ///     }
    ///     if sel.closed() {
    ///         // All channels are closed.
    ///         break;
    ///     }
    /// }
    /// */
    /// ```
    #[inline]
    pub fn closed(&mut self) -> bool {
        self.machine.closed()
    }

    /// Probes a *would block* case.
    ///
    /// This case fires when all operations in the loop would block.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (tx, rx) = unbounded::<i32>();
    ///
    /// let mut sel = Select::new();
    /// loop {
    ///     if let Ok(msg) = sel.recv(&rx) {
    ///         // The message was successfully received.
    ///         panic!();
    ///         break;
    ///     }
    ///     if sel.would_block() {
    ///         // All operation would block.
    ///         break;
    ///     }
    /// }
    /// ```
    #[inline]
    pub fn would_block(&mut self) -> bool {
        self.machine.would_block()
    }

    /// Probes a *timed out* case.
    ///
    /// This case fires when selection is blocked for longer than the specified timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (tx, rx) = unbounded::<i32>();
    ///
    /// let mut sel = Select::with_timeout(Duration::from_secs(1));
    /// loop {
    ///     if let Ok(msg) = sel.recv(&rx) {
    ///         // The message was successfully received.
    ///         panic!();
    ///         break;
    ///     }
    ///     if sel.timed_out() {
    ///         // Selection timed out.
    ///         break;
    ///     }
    /// }
    /// ```
    #[inline]
    pub fn timed_out(&mut self) -> bool {
        self.machine.timed_out()
    }
}

impl fmt::Debug for Select {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Select").finish()
    }
}

impl Default for Select {
    fn default() -> Select {
        Select::new()
    }
}
*/
