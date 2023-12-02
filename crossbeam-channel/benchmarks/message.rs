use std::fmt;

const LEN: usize = 1;

#[derive(Clone, Copy)]
pub(crate) struct Message(pub(crate) [usize; LEN]);

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Message")
    }
}

#[inline]
pub(crate) fn new(num: usize) -> Message {
    Message([num; LEN])
}
