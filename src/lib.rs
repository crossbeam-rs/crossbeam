//! Multi-producer multi-consumer channels for message passing.
//!
//! A channel is a concurrent FIFO queue used for passing messages between threads.
//!
//! Crossbeam's channel is an alternative to the [`std::sync::mpsc`] channel provided by the
//! standard library. It is an improvement in terms of performance, ergonomics, and features.
//!
//! Here's a simple example:
//!
//! ```
//! use crossbeam_channel as channel;
//!
//! // Create a channel of unbounded capacity.
//! let (s, r) = channel::unbounded();
//!
//! // Send a message into the channel.
//! s.send("Hello world!");
//!
//! // Receive the message from the channel.
//! assert_eq!(r.recv(), Some("Hello world!"));
//! ```
//!
//! # Types of channels
//!
//! A channel can be constructed by calling functions [`unbounded`] and [`bounded`]. The former
//! creates a channel of unbounded capacity (i.e. it can contain an arbitrary number of messages
//! at a same time), while the latter creates a channel of bounded capacity (i.e. there is a limit
//! to how many messages it can hold at a time).
//!
//! Both constructors return a pair of two values: a sender and a receiver. Senders and receivers
//! represent two opposite sides of a channel. Messages are sent into the channel using senders and
//! received from the channel using receivers.
//!
//! Creating an unbounded channel:
//!
//! ```
//! use crossbeam_channel as channel;
//!
//! // Create an unbounded channel.
//! let (s, r) = channel::unbounded();
//!
//! // Can send any number of messages into the channel without blocking.
//! for i in 0..1000 {
//!     s.send(i);
//! }
//! ```
//!
//! Creating a bounded channel:
//!
//! ```
//! # #[macro_use]
//! # extern crate crossbeam_channel;
//! # fn main() {
//! use crossbeam_channel as channel;
//!
//! // Create a channel that can hold at most 5 messages at a time.
//! let (s, r) = channel::bounded(5);
//!
//! // Can only send 5 messages without blocking.
//! for i in 0..5 {
//!     s.send(i);
//! }
//!
//! // Another call to `send` would block because the channel is full.
//! // s.send(5);
//! # }
//! ```
//!
//! A rather special case is a bounded, zero-capacity channel. This kind of channel cannot hold any
//! messages at all! In order to send a message through the channel, a sending thread and a
//! receiving thread have to pair up at the same time:
//!
//! ```
//! use std::thread;
//! use crossbeam_channel as channel;
//!
//! // Create a zero-capacity channel.
//! let (s, r) = channel::bounded(0);
//!
//! // Spawn a thread that sends a message into the channel.
//! // Sending blocks until a receive operation appears on the other side.
//! thread::spawn(move || s.send("Hi!"));
//!
//! // Receive the message.
//! // Receiving blocks until a send operation appears on the other side.
//! assert_eq!(r.recv(), Some("Hi!"));
//! ```
//!
//! # Sharing channels
//!
//! Senders and receivers can be either shared by reference or cloned and then sent to other
//! threads. There can be multiple senders and multiple receivers associated with the same channel.
//!
//! Sharing by reference:
//!
//! ```
//! # extern crate crossbeam_channel;
//! extern crate crossbeam;
//! # fn main() {
//! use crossbeam_channel as channel;
//!
//! let (s, r) = channel::unbounded();
//!
//! crossbeam::scope(|scope| {
//!     // Spawn a thread that sends one message and then receives one.
//!     scope.spawn(|| {
//!         s.send(1);
//!         r.recv().unwrap();
//!     });
//!
//!     // Spawn another thread that does the same thing.
//!     // Both closures capture `s` and `r` by reference.
//!     scope.spawn(|| {
//!         s.send(2);
//!         r.recv().unwrap();
//!     });
//! });
//!
//! # }
//! ```
//!
//! Sharing by sending clones:
//!
//! ```
//! use std::thread;
//! use crossbeam_channel as channel;
//!
//! let (s, r) = channel::unbounded();
//! let (s2, r2) = (s.clone(), r.clone());
//!
//! // Spawn a thread that sends one message and then receives one.
//! // Here, `s` and `r` are moved into the closure.
//! thread::spawn(move || {
//!     s.send(1);
//!     r.recv().unwrap();
//! });
//!
//! // Spawn another thread that does the same thing.
//! // Here, `s2` and `r2` are moved into the closure.
//! thread::spawn(move || {
//!     s2.send(2);
//!     r2.recv().unwrap();
//! });
//! ```
//!
//! # Closing
//!
//! As soon as all senders associated with a channel are dropped, it becomes closed. No more
//! messages can be sent, but the remaining messages can still be received. Receiving messages from
//! a closed channel never blocks.
//!
//! ```
//! use crossbeam_channel as channel;
//!
//! let (s, r) = channel::unbounded();
//! s.send(1);
//! s.send(2);
//! s.send(3);
//!
//! // The only sender is dropped, closing the channel.
//! drop(s);
//!
//! // The remaining messages can be received.
//! assert_eq!(r.recv(), Some(1));
//! assert_eq!(r.recv(), Some(2));
//! assert_eq!(r.recv(), Some(3));
//!
//! // There are no more messages in the channel.
//! assert!(r.is_empty());
//!
//! // Note that calling `r.recv()` will not block. Instead, it returns `None` immediately.
//! assert_eq!(r.recv(), None);
//! ```
//!
//! # Blocking and non-blocking operations
//!
//! If a bounded channel is full, a send operation will block until an empty slot in the channel
//! becomes available. Sending into an unbounded channel never blocks because there is always
//! enough space in it. A zero-capacity channel is always empty, and a send operation will block
//! until a receive operation appears on the other side of the channel.
//!
//! If a channel is empty, a receive operation will block until a message is sent into the channel.
//! In particular, a zero-capacity channel is always empty, and a receive operation will block
//! until a send operation appears on the other side of the channel.
//!
//! There is also a non-blocking method [`try_recv`], which receives a message if it is already
//! available, or returns `None` otherwise.
//!
//! ```
//! use crossbeam_channel as channel;
//!
//! let (s, r) = channel::bounded(1);
//!
//! // Send a message into the channel.
//! s.send("foo");
//!
//! // This call would block because the channel is full.
//! // s.send("bar");
//!
//! // Receive the message.
//! assert_eq!(r.recv(), Some("foo"));
//!
//! // This call would block because the channel is empty.
//! // r.recv();
//!
//! // Try receiving a message without blocking.
//! assert_eq!(r.try_recv(), None);
//!
//! // Close the channel.
//! drop(s);
//!
//! // This call doesn't block because the channel is now closed.
//! assert_eq!(r.recv(), None);
//! ```
//!
//! For greater control over blocking, consider using the [`select!`] macro.
//!
//! # Iteration
//!
//! A channel is essentially just a special kind of iterator, where items can be dynamically
//! produced by the sender side and consumed by the receiver side. Indeed, [`Receiver`] implements
//! the [`Iterator`] trait. Calling [`next`] on a receiver is equivalent to calling [`recv`].
//!
//! ```
//! use std::thread;
//! use crossbeam_channel as channel;
//!
//! let (s, r) = channel::unbounded();
//!
//! thread::spawn(move || {
//!     s.send(1);
//!     s.send(2);
//!     s.send(3);
//!     // `s` was moved into the closure so now it gets dropped, thus closing the channel.
//! });
//!
//! // Collect all messages from the channel. Note that the call to `collect` blocks until the
//! // channel becomes closed and empty, i.e. until `r.next()` returns `None`.
//! let v: Vec<_> = r.collect();
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! # Selection
//!
//! The [`select!`] macro allows one to block on multiple channel operations. Selection blocks
//! until any case from the specified set can run, and then it executes that case. Only one case in
//! a `select!` is executed. A random one is chosen if multiple channels operations are ready.
//!
//! An example of receiving one message from two channels, whichever becomes ready first:
//!
//! ```
//! # #[macro_use]
//! # extern crate crossbeam_channel;
//! # fn main() {
//! use std::thread;
//! use crossbeam_channel as channel;
//!
//! let (s1, r1) = channel::unbounded();
//! let (s2, r2) = channel::unbounded();
//!
//! thread::spawn(move || s1.send("foo"));
//! thread::spawn(move || s2.send("bar"));
//!
//! // Only one of these two receive operations will be executed.
//! select! {
//!     recv(r1, msg) => assert_eq!(msg, Some("foo")),
//!     recv(r2, msg) => assert_eq!(msg, Some("bar")),
//! }
//! # }
//! ```
//!
//! Send operations can be written inside [`select!`], too. It is also possible to specify what to
//! do in case none of the operations can be executed immediately, within a certain timeout, or
//! before a certain deadline.
//!
//! For more details, take a look at the documentation of [`select!`].
//!
//! [`std::sync::mpsc`]: https://doc.rust-lang.org/std/sync/mpsc/index.html
//! [`unbounded`]: fn.unbounded.html
//! [`bounded`]: fn.bounded.html
//! [`send`]: struct.Sender.html#method.send
//! [`try_recv`]: struct.Receiver.html#method.try_recv
//! [`recv`]: struct.Receiver.html#method.recv
//! [`next`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html#tymethod.next
//! [`Iterator`]: https://doc.rust-lang.org/std/iter/trait.Iterator.html
//! [`select!`]: macro.select.html
//! [`Sender`]: struct.Sender.html
//! [`Receiver`]: struct.Receiver.html

// TODO: explain comparison operators (with clones and multiple channels!)

extern crate crossbeam_epoch;
extern crate crossbeam_utils;
extern crate parking_lot;

// TODO: tokio benchmark
// TODO: test in select! where receiver/sender panics
// TODO: copy examples from chan crate

mod flavors;

#[doc(hidden)]
pub mod internal;

pub use internal::channel::{Receiver, Sender};
pub use internal::channel::{bounded, unbounded};
pub use internal::channel::{after, tick};
