use std::error;
use std::fmt;

/// An error returned from the [`Sender::send`] method.
///
/// A send operation can only fail if the receiving end of a channel is disconnected, implying that
/// the data could never be received. The error contains the data being sent as a payload so it can
/// be recovered.
///
/// [`Sender::send`]: struct.Sender.html#method.send
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

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

    /// This channel's receiving half has disconnected, so the data could not be sent. The data is
    /// returned back to the callee in this case.
    Disconnected(T),
}

/// This enumeration is the list of possible errors that made [`send_timeout`] unable to return
/// data when called. This can occur with bounded channels only.
///
/// [`send_timeout`]: struct.Sender.html#method.send_timeout
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SendTimeoutError<T> {
    /// This channel is currently full, but the receivers have not yet disconnected.
    Timeout(T),

    /// The channel's receiving half has become disconnected.
    Disconnected(T),
}

/// An error returned from the [`Select::send`] method.
///
/// This error occurs when the selection case doesn't send a message into the channel. Note that
/// cases enumerated in a selection loop are sometimes simply skipped, so they might fail even if
/// the channel is currently not full.
///
/// [`Select::send`]: struct.Select.html#method.send
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SelectSendError<T>(pub T);

/// An error returned from the [`Receiver::recv`] method.
///
/// The [`recv`] operation can only fail if the sending half of a channel is disconnected and the
/// channel is empty, implying that no further messages will ever be received.
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
    /// This channel is currently empty, but the senders have not yet disconnected, so data may yet
    /// become available.
    ///
    /// If this is a zero-capacity channel, then the error indicates that there was no sender
    /// available to at the time.
    Empty,

    /// The channel's sending half has become disconnected, and there will never be any more data
    /// received on it.
    Disconnected,
}

/// This enumeration is the list of possible errors that made [`recv_timeout`] unable to return
/// data when called. This can occur with both bounded and unbounded channels.
///
/// [`recv_timeout`]: struct.Receiver.html#method.recv_timeout
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum RecvTimeoutError {
    /// This channel is currently empty, but the senders have not yet disconnected, so data may yet
    /// become available.
    Timeout,

    /// The channel's sending half has become disconnected, and there will never be any more data
    /// received on it.
    Disconnected,
}

/// An error returned from the [`Select::recv`] method.
///
/// This error occurs when the selection case doesn't receive a message from the channel. Note that
/// cases enumerated in a selection loop are sometimes simply skipped, so they might fail even if
/// the channel is currently not empty.
///
/// [`Select::recv`]: struct.Select.html#method.recv
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct SelectRecvError;

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "SendError(..)".fmt(f)
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "sending on a closed channel".fmt(f)
    }
}

impl<T: Send> error::Error for SendError<T> {
    fn description(&self) -> &str {
        "sending on a closed channel"
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl<T> SendError<T> {
    /// Unwraps the value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let (tx, rx) = channel::unbounded();
    /// drop(rx);
    ///
    /// if let Err(err) = tx.send("foo") {
    ///     assert_eq!(err.into_inner(), "foo");
    /// }
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => "Full(..)".fmt(f),
            TrySendError::Disconnected(..) => "Disconnected(..)".fmt(f),
        }
    }
}

impl<T> fmt::Display for TrySendError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TrySendError::Full(..) => "sending on a full channel".fmt(f),
            TrySendError::Disconnected(..) => "sending on a closed channel".fmt(f),
        }
    }
}

impl<T: Send> error::Error for TrySendError<T> {
    fn description(&self) -> &str {
        match *self {
            TrySendError::Full(..) => "sending on a full channel",
            TrySendError::Disconnected(..) => "sending on a closed channel",
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
    /// let (tx, rx) = channel::bounded(0);
    ///
    /// if let Err(err) = tx.try_send("foo") {
    ///     assert_eq!(err.into_inner(), "foo");
    /// }
    /// ```
    pub fn into_inner(self) -> T {
        match self {
            TrySendError::Full(v) => v,
            TrySendError::Disconnected(v) => v,
        }
    }
}

impl<T> fmt::Debug for SendTimeoutError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "SendTimeoutError(..)".fmt(f)
    }
}

impl<T> fmt::Display for SendTimeoutError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            SendTimeoutError::Timeout(..) => "timed out waiting on channel".fmt(f),
            SendTimeoutError::Disconnected(..) => "sending on a closed channel".fmt(f),
        }
    }
}

impl<T: Send> error::Error for SendTimeoutError<T> {
    fn description(&self) -> &str {
        "sending on a closed channel"
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl<T> SendTimeoutError<T> {
    /// Unwraps the value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// let (tx, rx) = channel::unbounded();
    ///
    /// if let Err(err) = tx.send_timeout("foo", Duration::from_secs(0)) {
    ///     assert_eq!(err.into_inner(), "foo");
    /// }
    /// ```
    pub fn into_inner(self) -> T {
        match self {
            SendTimeoutError::Timeout(v) => v,
            SendTimeoutError::Disconnected(v) => v,
        }
    }
}

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
    /// use channel::Select;
    ///
    /// let (tx, rx) = channel::unbounded();
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

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "receiving on a closed channel".fmt(f)
    }
}

impl error::Error for RecvError {
    fn description(&self) -> &str {
        "receiving on a closed channel"
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            TryRecvError::Empty => "receiving on an empty channel".fmt(f),
            TryRecvError::Disconnected => "receiving on a closed channel".fmt(f),
        }
    }
}

impl error::Error for TryRecvError {
    fn description(&self) -> &str {
        match *self {
            TryRecvError::Empty => "receiving on an empty channel",
            TryRecvError::Disconnected => "receiving on a closed channel",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

impl fmt::Display for RecvTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            RecvTimeoutError::Timeout => "timed out waiting on channel".fmt(f),
            RecvTimeoutError::Disconnected => "channel is empty and sending half is closed".fmt(f),
        }
    }
}

impl error::Error for RecvTimeoutError {
    fn description(&self) -> &str {
        match *self {
            RecvTimeoutError::Timeout => "timed out waiting on channel",
            RecvTimeoutError::Disconnected => "channel is empty and sending half is closed",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        None
    }
}

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
