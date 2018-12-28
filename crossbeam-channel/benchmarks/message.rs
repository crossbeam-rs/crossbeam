use std::fmt;

const LEN: usize = 1;

#[derive(Clone, Copy)]
pub struct Message(pub [usize; LEN]);

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Message")
    }
}

#[inline]
pub fn new(num: usize) -> Message {
    Message([num; LEN])
}
