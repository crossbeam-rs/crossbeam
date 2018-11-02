//! Multi-producer multi-consumer channels for message passing.
//!
//! This library is an alternative to [`std::sync::mpsc`] with more features and better
//! performance.
//!
//! # Hello, world!
//!
//! ```
//! use crossbeam_channel::unbounded;
//!
//! // Create a channel of unbounded capacity.
//! let (s, r) = unbounded();
//!
//! // Send a message into the channel.
//! s.send("Hello, world!").unwrap();
//!
//! // Receive the message from the channel.
//! assert_eq!(r.recv(), Ok("Hello, world!"));
//! ```
//!
//! # Types of channels
//!
//! Channels are created using two functions:
//!
//! * [`bounded`] creates a channel of bounded capacity, i.e. there is a limit to how many messages
//!   it can hold at a time.
//!
//! * [`unbounded`] creates a channel of unbounded capacity, i.e. it can contain any number of
//!   messages at a time.
//!
//! Both functions return two handles: a sender and a receiver. Senders and receivers represent
//! two opposite sides of a channel.
//!
//! Creating a bounded channel:
//!
//! ```
//! use crossbeam_channel::bounded;
//!
//! // Create a channel that can hold at most 5 messages at a time.
//! let (s, r) = bounded(5);
//!
//! // Can only send 5 messages without blocking.
//! for i in 0..5 {
//!     s.send(i).unwrap();
//! }
//!
//! // Another call to `send` would block because the channel is full.
//! // s.send(5).unwrap();
//! ```
//!
//! Creating an unbounded channel:
//!
//! ```
//! use crossbeam_channel::unbounded;
//!
//! // Create an unbounded channel.
//! let (s, r) = unbounded();
//!
//! // Can send any number of messages into the channel without blocking.
//! for i in 0..1000 {
//!     s.send(i).unwrap();
//! }
//! ```
//!
//! A special case is zero-capacity channel, also known as *rendezvous* channel. Such a channel
//! cannot hold any messages. Instead, send and receive operations must appear at the same time in
//! order to pair up and pass the message over:
//!
//! ```
//! use std::thread;
//! use crossbeam_channel::bounded;
//!
//! // Create a zero-capacity channel.
//! let (s, r) = bounded(0);
//!
//! // Spawn a thread that sends a message into the channel.
//! // Sending blocks until a receive operation appears on the other side.
//! thread::spawn(move || s.send("Hi!").unwrap());
//!
//! // Receive the message.
//! // Receiving blocks until a send operation appears on the other side.
//! assert_eq!(r.recv(), Ok("Hi!"));
//! ```
//!
//! # Senders and receivers
//!
//! Even though senders and receivers can be shared among threads by reference, sometimes it's more
//! convenient to clone them and send clones to other threads:
//!
//! ```
//! use std::thread;
//! use crossbeam_channel::unbounded;
//!
//! let (s1, r1) = unbounded();
//! let (s2, r2) = (s1.clone(), r1.clone());
//!
//! // Spawn a thread that sends one message and then receives one.
//! thread::spawn(move || {
//!     s1.send(1).unwrap();
//!     r1.recv().unwrap();
//! });
//!
//! // Spawn another thread that receives a message and then sends one.
//! thread::spawn(move || {
//!     r2.recv().unwrap();
//!     s2.send(2).unwrap();
//! });
//! ```
//!
//! Note that cloning only creates a new reference to the same sending or receiving side. Cloning
//! does not create a separate stream of messages in any way:
//!
//! ```
//! use crossbeam_channel::unbounded;
//!
//! let (s1, r1) = unbounded();
//! let (s2, r2) = (s1.clone(), r1.clone());
//! let (s3, r3) = (s2.clone(), r2.clone());
//!
//! s1.send(10).unwrap();
//! s2.send(20).unwrap();
//! s3.send(30).unwrap();
//!
//! assert_eq!(r3.recv(), Ok(10));
//! assert_eq!(r1.recv(), Ok(20));
//! assert_eq!(r2.recv(), Ok(30));
//! ```
//!
//! # Disconnection
//!
//! When all senders or all receivers associated with a channel get dropped, the channel becomes
//! disconnected. No more messages can be sent, but any remaining messages can still be received.
//! Send and receive operations on a disconnected channel never block.
//!
//! ```
//! use crossbeam_channel::{unbounded, RecvError};
//!
//! let (s, r) = unbounded();
//! s.send(1).unwrap();
//! s.send(2).unwrap();
//! s.send(3).unwrap();
//!
//! // The only sender is dropped, disconnecting the channel.
//! drop(s);
//!
//! // The remaining messages can be received.
//! assert_eq!(r.recv(), Ok(1));
//! assert_eq!(r.recv(), Ok(2));
//! assert_eq!(r.recv(), Ok(3));
//!
//! // There are no more messages in the channel.
//! assert!(r.is_empty());
//!
//! // Note that calling `r.recv()` will not block.
//! // Instead, `Err(RecvError)` is returned immediately.
//! assert_eq!(r.recv(), Err(RecvError));
//! ```
//!
//! # Blocking operations
//!
//! Send and receive operations come in three flavors:
//!
//! * Non-blocking (returns immediately with success or failure).
//! * Blocking (waits until the operation succeeds or the channel becomes disconnected).
//! * Blocking with a timeout (blocks only for a certain duration of time).
//!
//! Here's a simple example showing the difference between non-blocking and blocking operations:
//!
//! ```
//! use crossbeam_channel::{bounded, RecvError, TryRecvError};
//!
//! let (s, r) = bounded(1);
//!
//! // Send a message into the channel.
//! s.send("foo").unwrap();
//!
//! // This call would block because the channel is full.
//! // s.send("bar").unwrap();
//!
//! // Receive the message.
//! assert_eq!(r.recv(), Ok("foo"));
//!
//! // This call would block because the channel is empty.
//! // r.recv();
//!
//! // Try receiving a message without blocking.
//! assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
//!
//! // Disconnect the channel.
//! drop(s);
//!
//! // This call doesn't block because the channel is now disconnected.
//! assert_eq!(r.recv(), Err(RecvError));
//! ```
//!
//! # Iteration
//!
//! Receivers can be used as iterators. For example, method [`iter`] creates an iterator that
//! receives messages until the channel becomes empty and disconnected. Note that iteration may
//! block waiting for next message to arrive.
//!
//! ```
//! use std::thread;
//! use crossbeam_channel::unbounded;
//!
//! let (s, r) = unbounded();
//!
//! thread::spawn(move || {
//!     s.send(1).unwrap();
//!     s.send(2).unwrap();
//!     s.send(3).unwrap();
//!     drop(s); // Disconnect the channel.
//! });
//!
//! // Collect all messages from the channel.
//! //
//! // Note that the call to `collect` blocks until the sender is dropped.
//! let v: Vec<_> = r.iter().collect();
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! A non-blocking iterator can be created using [`try_iter`], which receives messages if they're
//! available without blocking:
//!
//! ```
//! use crossbeam_channel::unbounded;
//!
//! let (s, r) = unbounded();
//! s.send(1).unwrap();
//! s.send(2).unwrap();
//! s.send(3).unwrap();
//! // No need to drop the sender.
//!
//! // Receive all messages currently in the channel.
//! let v: Vec<_> = r.try_iter().collect();
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! # Selection
//!
//! The [`select`] macro allows you to define a set of channel operations, block until any one of
//! them becomes ready, and finally execute it. If multiple operations are ready at the same time,
//! a random one among them is selected.
//!
//! It is also possible to define a `default` case that gets executed if none of the operations are
//! ready, either right away or for a certain duration of time.
//!
//! An example of receiving a message from two channels, whichever becomes ready first:
//!
//! ```
//! # #[macro_use]
//! # extern crate crossbeam_channel;
//! # fn main() {
//! use std::thread;
//! use std::time::Duration;
//! use crossbeam_channel::unbounded;
//!
//! let (s1, r1) = unbounded();
//! let (s2, r2) = unbounded();
//!
//! thread::spawn(move || s1.send(10).unwrap());
//! thread::spawn(move || s2.send(20).unwrap());
//!
//! // Only one of these two receive operations will be executed.
//! select! {
//!     recv(r1) -> msg => assert_eq!(msg, Ok(10)),
//!     recv(r2) -> msg => assert_eq!(msg, Ok(20)),
//!     default(Duration::from_secs(1)) => println!("timed out"),
//! }
//! # }
//! ```
//!
//! If you need to dynamically add channel operations rather than define them statically inside the
//! macro, use [`Select`] instead. The [`select`] macro is just a wrapper around [`Select`] with
//! more pleasant interface.
//!
//! # Additional tools
//!
//! There are two functions that create special kinds of channels:
//!
//! * [`after`] creates a channel that delivers a single message after a certain duration of time.
//! * [`tick`] creates a channel that delivers messages periodically.
//!
//! These functions return a receiver only because messages get automatically sent into such
//! channels. They are sometimes more convenient and more efficient than manually spawning a thread
//! that sleeps and sends messages into the channel.
//!
//! [`std::sync::mpsc`]: https://doc.rust-lang.org/std/sync/mpsc/index.html
//! [`unbounded`]: fn.unbounded.html
//! [`bounded`]: fn.bounded.html
//! [`after`]: fn.after.html
//! [`tick`]: fn.tick.html
//! [`send`]: struct.Sender.html#method.send
//! [`recv`]: struct.Receiver.html#method.recv
//! [`iter`]: struct.Receiver.html#method.iter
//! [`try_iter`]: struct.Receiver.html#method.try_iter
//! [`select`]: macro.select.html
//! [`Select`]: struct.Select.html
//! [`Sender`]: struct.Sender.html
//! [`Receiver`]: struct.Receiver.html

extern crate crossbeam_epoch;
extern crate crossbeam_utils;
extern crate rand;
extern crate parking_lot;
extern crate smallvec;

mod channel;
mod context;
mod err;
mod flavors;
mod select;
mod utils;
mod waker;

pub use channel::{Receiver, Sender};
pub use channel::{bounded, unbounded};
pub use channel::{after, tick};
pub use channel::{IntoIter, Iter, TryIter};

pub use select::{Select, SelectedCase};

pub use err::{RecvError, RecvTimeoutError, TryRecvError};
pub use err::{SendError, SendTimeoutError, TrySendError};
pub use err::{SelectTimeoutError, TrySelectError};
