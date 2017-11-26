//! Multi-producer multi-consumer channels for message passing.
//!
//! Channels are concurrent FIFO queues used for passing messages between threads.
//!
//! Crossbeam's channels are an alternative to the [`std::sync::mpsc`] channels provided by the
//! standard library. They are an improvement in pretty much all aspects: ergonomics, flexibility,
//! features, performance.
//!
//! # Types of channels
//!
//! A channel can be constructed by calling functinos [`unbounded`] and [`bounded`]. The former
//! creates a channel of unbounded capacity (i.e. it can contain an arbitrary number of messages),
//! while the latter creates a channel of bounded capacity (i.e. there is a limit to how many
//! messages it can hold at a time).
//!
//! Both constructors returns a pair of two values: a sender and a receiver. Senders and receivers
//! represent two opposite sides of a channel. Messages are sent using senders and received using
//! receivers.
//!
//! Creating an unbounded channel:
//!
//! ```
//! // Create an unbounded channel.
//! let (tx, rx) = channel::unbounded();
//!
//! // Can send an arbitrarily large number of messages.
//! for i in 0..1000 {
//!     tx.try_send(i).unwrap();
//! }
//! ```
//!
//! Creating a bounded channel:
//!
//! ```
//! // Create a channel that can hold at most 5 messages at a time.
//! let (tx, rx) = channel::bounded(5);
//!
//! // Can send only 5 messages.
//! for i in 0..5 {
//!     tx.try_send(i).unwrap();
//! }
//!
//! // An attempt to send one more message will fail.
//! assert!(tx.try_send(5).is_err());
//! ```
//!
//! An interesting special case is a bounded, zero-capacity channel. This kind of channel cannot
//! hold any messages at all! In order to send a message through the channel, another thread must
//! be waiting at the other end of it at the same time:
//!
//! ```
//! use std::thread;
//!
//! // Create a zero-capacity channel.
//! let (tx, rx) = channel::bounded(0);
//!
//! // Spawn a thread that sends a message into the channel.
//! thread::spawn(move || tx.send("Hi!").unwrap());
//!
//! // Receive the message.
//! assert_eq!(rx.recv(), Ok("Hi!"));
//! ```
//!
//! # Sharing channels
//!
//! Senders and receivers can be either shared by reference or cloned and then sent to other
//! threads. Feel free to use any of these two approaches as you like.
//!
//! Sharing by reference:
//!
//! ```
//! extern crate channel;
//! extern crate crossbeam_utils;
//!
//! let (tx, rx) = channel::unbounded();
//!
//! crossbeam_utils::scoped::scope(|s| {
//!     // Spawn a thread that sends one message and then receives one.
//!     s.spawn(|| {
//!         tx.send(1).unwrap();
//!         rx.recv().unwrap();
//!     });
//!
//!     // Spawn another thread that does the same thing.
//!     // Both closures capture `tx` and `rx` by reference.
//!     s.spawn(|| {
//!         tx.send(2).unwrap();
//!         rx.recv().unwrap();
//!     });
//! });
//! ```
//!
//! Sharing by sending:
//!
//! ```
//! use std::thread;
//!
//! let (tx, rx) = channel::unbounded();
//! let (tx2, rx2) = (tx.clone(), rx.clone());
//!
//! // Spawn a thread that sends one message and then receives one.
//! // Here, `tx` and `rx` are moved into the closure (sent into the thread).
//! thread::spawn(move || {
//!     tx.send(1).unwrap();
//!     rx.recv().unwrap();
//! });
//!
//! // Spawn another thread that does the same thing.
//! // Here, `tx2` and `rx2` are moved into the closure (sent into the thread).
//! thread::spawn(move || {
//!     tx2.send(2).unwrap();
//!     rx2.recv().unwrap();
//! });
//! ```
//!
//! # Disconnection
//!
//! As soon as all senders or all receivers associated with a channel are dropped, it becomes
//! disconnected. Messages cannot be sent into a disconnected channel anymore, but the remaining
//! messages can still be received.
//!
//! ```
//! use channel::TrySendError;
//!
//! let (tx, rx) = channel::unbounded();
//!
//! // The only receiver is dropped, disconnecting the channel.
//! drop(rx);
//!
//! // Attempting to send a message will result in an error.
//! assert_eq!(tx.try_send("hello"), Err(TrySendError::Disconnected("hello")));
//! ```
//!
//! ```
//! use channel::TryRecvError;
//!
//! let (tx, rx) = channel::unbounded();
//! tx.try_send(1).unwrap();
//! tx.try_send(2).unwrap();
//! tx.try_send(3).unwrap();
//!
//! // The only sender is dropped, disconnecting the channel.
//! drop(tx);
//!
//! // The remaining messages can be received.
//! assert_eq!(rx.try_recv(), Ok(1));
//! assert_eq!(rx.try_recv(), Ok(2));
//! assert_eq!(rx.try_recv(), Ok(3));
//!
//! // However, attempting to receive another message will result in an error.
//! assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
//! ```
//!
//! # Blocking and non-blocking operations
//!
//! Send and receive operations come in three variants:
//!
//! 1. Non-blocking: [`try_send`] and [`try_recv`].
//! 2. Blocking: [`try_send`] and [`try_recv`].
//! 3. Blocking with a timeout: [`send_timeout`] and [`recv_timeout`].
//!
//! The non-blocking variant attempts to perform the operation, but doesn't block the current
//! thread on failure (e.g. if receiving a message from an empty channel).
//!
//! The blocking variant will wait until the operation can be performed or the channel becomes
//! disconnected.
//!
//! Blocking with a timeout does the same thing, but blocks the current thread only for a limited
//! amount time.
//!
//! # Iteration
//!
//! Receivers can be turned into iterators. For example, calling [`iter`] creates an iterator that
//! returns messages until the channel is disconnected. Note that iteration may block while waiting
//! for the next message.
//!
//! ```
//! let (tx, rx) = channel::unbounded();
//! tx.send(1).unwrap();
//! tx.send(2).unwrap();
//! tx.send(3).unwrap();
//!
//! // Drop the sender in order to disconnect the channel.
//! drop(tx);
//!
//! // Receive all remaining messages.
//! let v: Vec<_> = rx.iter().collect();
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! By calling [`try_iter`] it is also possible to create an iterator that returns messages until
//! the channel is empty. This iterator will never block the current thread.
//!
//! ```
//! let (tx, rx) = channel::unbounded();
//! tx.send(1).unwrap();
//! tx.send(2).unwrap();
//! tx.send(3).unwrap();
//! // No need to drop the sender.
//!
//! // Receive all messages currently in the channel.
//! let v: Vec<_> = rx.try_iter().collect();
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! Finally, there is the [`into_iter`] method, which is equivalent to [`iter`], except it takes
//! ownership of the receiver instead of borrowing it.
//!
//! # Selection
//!
//! Selection allows you to declare a set of operations on channels and perform exactly one of
//! them, whichever becomes ready first, possibly blocking until that happens.
//!
//! For example, selection can be used to receive a message from one of the two channels, blocking
//! until a message appears on either of them:
//!
//! ```
//! # #[macro_use]
//! # extern crate channel;
//! # fn main() {
//!
//! use std::thread;
//!
//! let (tx1, rx1) = channel::unbounded();
//! let (tx2, rx2) = channel::unbounded();
//!
//! thread::spawn(move || tx1.send("foo").unwrap());
//! thread::spawn(move || tx2.send("bar").unwrap());
//!
//! select_loop! {
//!     recv(rx1, msg) => {
//!         println!("Received a message from the first channel: {}", msg);
//!     }
//!     recv(rx2, msg) => {
//!         println!("Received a message from the second channel: {}", msg);
//!     }
//! }
//!
//! # }
//! ```
//!
//! The syntax of [`select_loop!`] is very similar to the one used by `match`.
//!
//! Here is another, more complicated example of selection. Here we are selecting over two
//! operations on the opposite ends of the same channel: a send and a receive operation.
//!
//! ```
//! # #[macro_use]
//! # extern crate channel;
//! # fn main() {
//!
//! use channel::{Sender, Receiver, Select};
//! use std::thread;
//!
//! // Either send my name into the channel or receive someone else's, whatever happens first.
//! fn seek<'a>(name: &'a str, tx: Sender<&'a str>, rx: Receiver<&'a str>) {
//!     select_loop! {
//!         recv(rx, peer) => println!("{} received a message from {}.", name, peer),
//!         send(tx, name) => {},
//!     }
//! }
//!
//! let (tx, rx) = channel::bounded(1); // Make room for one unmatched send.
//!
//! // Pair up five people by exchanging messages over the channel.
//! // Since there is an odd number of them, one person won't have its match.
//! ["Anna", "Bob", "Cody", "Dave", "Eva"].iter()
//!     .map(|name| {
//!         let tx = tx.clone();
//!         let rx = rx.clone();
//!         thread::spawn(move || seek(name, tx, rx))
//!     })
//!     .collect::<Vec<_>>()
//!     .into_iter()
//!     .for_each(|t| t.join().unwrap());
//!
//! // Let's send a message to the remaining person who doesn't have a match.
//! if let Ok(name) = rx.try_recv() {
//!     println!("No one received {}’s message.", name);
//! }
//!
//! # }
//! ```
//!
//! For more details, take a look at the documentation of [`select_loop!`].
//!
//! If you need a more powerful interface that allows selecting over a dynamic set of channel
//! operations, use [`Select`].
//!
//! [`std::sync::mpsc`]: https://doc.rust-lang.org/std/sync/mpsc/index.html
//! [`unbounded`]: fn.unbounded.html
//! [`bounded`]: fn.bounded.html
//! [`try_send`]: struct.Sender.html#method.try_send
//! [`send`]: struct.Sender.html#method.send
//! [`send_timeout`]: struct.Sender.html#method.send_timeout
//! [`try_recv`]: struct.Receiver.html#method.try_recv
//! [`recv`]: struct.Receiver.html#method.recv
//! [`recv_timeout`]: struct.Receiver.html#method.recv_timeout
//! [`iter`]: struct.Receiver.html#method.iter
//! [`try_iter`]: struct.Receiver.html#method.try_iter
//! [`into_iter`]: struct.Receiver.html#method.into_iter
//! [`select_loop!`]: macro.select_loop.html
//! [`Select`]: struct.Select.html

#![cfg_attr(feature = "nightly", feature(hint_core_should_pause))]

extern crate crossbeam_epoch;
extern crate crossbeam_utils;
extern crate parking_lot;
extern crate rand;

mod channel;
mod err;
mod exchanger;
mod flavors;
mod monitor;
mod select;
mod util;

pub use channel::{bounded, unbounded};
pub use channel::{Receiver, Sender};
pub use channel::{IntoIter, Iter, TryIter};
pub use err::{RecvError, RecvTimeoutError, TryRecvError};
pub use err::{SendError, SendTimeoutError, TrySendError};
pub use err::{SelectRecvError, SelectSendError};
pub use select::Select;
