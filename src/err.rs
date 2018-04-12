use std::error;
use std::fmt;

/// This enumeration is the list of the possible error outcomes for the [`try_send`] method.
///
/// [`try_send`]: struct.Sender.html#method.try_send
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T> {
    /// The data could not be sent on the channel because it would require that the callee block to
    /// send the data.
    ///
    /// If this is a zero-capacity channel, then the error indicates that there was no receiver
    /// available to receive the message at the time.
    Full(T),

    /// This channel is closed, so the data could not be sent. The data is returned back to the
    /// callee in this case.
    Closed(T),
}

/// An error returned from the [`Receiver::recv`] method.
///
/// The [`recv`] operation can only fail if the channel is closed and empty, implying that no
/// further messages will ever be received.
///
/// [`Receiver::recv`]: struct.Receiver.html#method.recv
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

/// This enumeration is the list of the possible reasons that [`try_recv`] could not return data
/// when called. This can occur with both bounded and unbounded channels.
///
/// [`try_recv`]: struct.Receiver.html#method.recv
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError {
    /// This channel is currently empty, but not yet closed, so data may yet become available.
    ///
    /// If this is a zero-capacity channel, then the error indicates that there was no sender
    /// available to at the time.
    Empty,

    /// The channel is closed, and there will never be any more data received on it.
    Closed,
}

/*
/// An error returned from the [`Select::recv`] method.
///
/// This error occurs when the selection case doesn't receive a message from the channel. Note that
/// cases enumerated in a selection loop are sometimes simply skipped, so they might fail even if
/// the channel is currently not empty.
///
/// [`Select::recv`]: struct.Select.html#method.recv
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct SelectRecvError;
*/

impl<T> fmt::Debug for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => "Full(..)".fmt(f),
            TrySendError::Closed(..) => "Closed(..)".fmt(f),
        }
    }
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => "sending on a full channel".fmt(f),
            TrySendError::Closed(..) => "sending on a closed channel".fmt(f),
        }
    }
}

impl<T: Send> error::Error for TrySendError<T> {
    fn description(&self) -> &str {
        match *self {
            TrySendError::Full(..) => "sending on a full channel",
            TrySendError::Closed(..) => "sending on a closed channel",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl<T> TrySendError<T> {
    /// Unwraps the value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use crossbeam_channel::bounded;
    ///
    /// let (tx, rx) = bounded(0);
    ///
    /// if let Err(err) = tx.try_send("foo") {
    ///     assert_eq!(err.into_inner(), "foo");
    /// }
    /// ```
    pub fn into_inner(self) -> T {
        match self {
            TrySendError::Full(v) => v,
            TrySendError::Closed(v) => v,
        }
    }
}

/*
impl<T: Send> fmt::Debug for SelectSendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "SelectSendError(..)".fmt(f)
    }
}

impl<T: Send> fmt::Display for SelectSendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "selection `send` case is not ready".fmt(f)
    }
}

impl<T: Send> error::Error for SelectSendError<T> {
    fn description(&self) -> &str {
        "selection `send` case is not ready"
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl<T> SelectSendError<T> {
    /// Unwraps the value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use crossbeam_channel::{unbounded, Select};
    ///
    /// let (tx, rx) = unbounded();
    ///
    /// let mut msg = "message".to_string();
    /// let mut sel = Select::new();
    /// loop {
    ///     if let Err(err) = sel.send(&tx, msg) {
    ///         msg = err.into_inner();
    ///     } else {
    ///         break;
    ///     }
    /// }
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}
*/

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "receiving on an empty and closed channel".fmt(f)
    }
}

impl error::Error for RecvError {
    fn description(&self) -> &str {
        "receiving on an empty and closed channel"
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TryRecvError::Empty => "receiving on an empty channel".fmt(f),
            TryRecvError::Closed => "receiving on an empty and closed channel".fmt(f),
        }
    }
}

impl error::Error for TryRecvError {
    fn description(&self) -> &str {
        match *self {
            TryRecvError::Empty => "receiving on an empty channel",
            TryRecvError::Closed => "receiving on an empty and closed channel",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl From<RecvError> for TryRecvError {
    fn from(err: RecvError) -> TryRecvError {
        match err {
            RecvError => TryRecvError::Closed,
        }
    }
}

/*
impl fmt::Display for SelectRecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "selection `recv` case is not ready".fmt(f)
    }
}

impl error::Error for SelectRecvError {
    fn description(&self) -> &str {
        "selection `recv` case is not ready"
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}
*/
