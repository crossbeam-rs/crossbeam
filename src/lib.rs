#![cfg_attr(feature = "nightly", feature(hint_core_should_pause))]

extern crate coco;
extern crate crossbeam;
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
