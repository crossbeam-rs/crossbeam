//! Multi-producer multi-consumer channels for message passing.
//!
//! Crossbeam's channels are an alternative to the [`std::sync::mpsc`] channels provided by the
//! standard library. They are an improvement in terms of performance, ergonomics, and features.
//!
//! Here's a quick example:
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
//! A channel can be created by calling [`bounded`] or [`unbounded`]. The former creates a channel
//! of bounded capacity (i.e. there is a limit to how many messages it can hold), while the latter
//! creates a channel of unbounded capacity (i.e. it can contain an arbitrary number of messages).
//!
//! Both functions return two handles: a sender and a receiver. Senders and receivers represent
//! two opposite sides of a channel. Messages are sent into the channel using senders and received
//! using receivers.
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
//!     scope.spawn(|| {
//!         s.send(2);
//!         r.recv().unwrap();
//!     });
//! });
//!
//! # }
//! ```
//!
//! Sharing by cloning:
//!
//! ```
//! use std::thread;
//! use crossbeam_channel as channel;
//!
//! let (s1, r1) = channel::unbounded();
//! let (s2, r2) = (s1.clone(), r1.clone());
//!
//! // Spawn a thread that sends one message and then receives one.
//! thread::spawn(move || {
//!     s1.send(1);
//!     r1.recv().unwrap();
//! });
//!
//! // Spawn another thread that receives a message and then sends one.
//! thread::spawn(move || {
//!     r2.recv().unwrap();
//!     s2.send(2);
//! });
//! ```
//!
//! # Closing channels
//!
//! When all senders associated with a channel get dropped, the channel becomes closed. No more
//! messages can be sent, but any remaining messages can still be received. Receive operations on a
//! closed channel never block, even if the channel is empty.
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
//! // Note that calling `r.recv()` will not block.
//! // Instead, `None` is returned immediately.
//! assert_eq!(r.recv(), None);
//! ```
//!
//! # Blocking and non-blocking operations
//!
//! Sending a message into a full bounded channel will block until an empty slot in the channel
//! becomes available. Sending into an unbounded channel never blocks because there is always
//! enough space in it. Zero-capacity channels are always empty, and sending blocks until a receive
//! operation appears on the other side of the channel.
//!
//! Receiving from an empty channel blocks until a message is sent into the channel or the channel
//! becomes closed. Zero-capacity channels are always empty, and receiving blocks until a send
//! operation appears on the other side of the channel or it becomes closed.
//!
//! There is also a non-blocking method [`try_recv`], which receives a message if it is immediately
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
//! A channel is a special kind of iterator, where items can be dynamically produced by senders and
//! consumed by receivers. Indeed, [`Receiver`] implements the [`Iterator`] trait, and calling
//! [`next`] is equivalent to calling [`recv`].
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
//!     // `s` was moved into the closure so now it gets dropped,
//!     // thus closing the channel.
//! });
//!
//! // Collect all messages from the channel.
//! //
//! // Note that the call to `collect` blocks until the channel becomes
//! // closed and empty, i.e. until `r.next()` returns `None`.
//! let v: Vec<_> = r.collect();
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! # Select
//!
//! The [`select!`] macro allows declaring a set of channel operations and blocking until any one
//! of them becomes ready. Finally, one of the operations is executed. If multiple operations
//! are ready at the same time, a random one is chosen. It is also possible to declare a `default`
//! case that gets executed if none of the operations are initially ready.
//!
//! An example of receiving a message from two channels, whichever becomes ready first:
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
//! For more details, take a look at the documentation for [`select!`].
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

extern crate crossbeam_epoch;
extern crate crossbeam_utils;
extern crate rand;
extern crate parking_lot;

mod flavors;

#[doc(hidden)]
pub mod internal;

pub use internal::channel::{Receiver, Sender};
pub use internal::channel::{bounded, unbounded};
pub use internal::channel::{after, tick};
pub use internal::select::Select;
