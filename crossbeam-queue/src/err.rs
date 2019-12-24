use core::fmt;

/// Error which occurs when popping from an empty queue.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PopError;

impl fmt::Debug for PopError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "PopError".fmt(f)
    }
}

impl fmt::Display for PopError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "popping from an empty queue".fmt(f)
    }
}

#[cfg(features = "std")]
impl std::error::Error for PopError {
    fn description(&self) -> &str {
        "popping from an empty queue"
    }
}

/// Error which occurs when pushing into a full queue.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PushError<T>(pub T);

impl<T> fmt::Debug for PushError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "PushError(..)".fmt(f)
    }
}

impl<T> fmt::Display for PushError<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        "pushing into a full queue".fmt(f)
    }
}

#[cfg(features = "std")]
impl<T: Send> std::error::Error for PushError<T> {
    fn description(&self) -> &str {
        "pushing into a full queue"
    }
}
