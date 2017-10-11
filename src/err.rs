use std::error;
use std::fmt;

/// An error returned from the [`Sender::send`] method on channels.
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
    /// If this is a bounded positive-capacity channel, then it is full at this time. If this is a
    /// zero-capacity channel, then there is no [`Receiver`] available to acquire the data.
    ///
    /// [`Receiver`]: struct.Receiver.html
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

/// An error returned from the [`recv`] method on a [`Receiver`].
///
/// The [`recv`] operation can only fail if the sending half of a channel is disconnected, implying
/// that no further messages will ever be received.
///
/// [`recv`]: struct.Receiver.html#method.recv
/// [`Receiver`]: struct.Receiver.html
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
