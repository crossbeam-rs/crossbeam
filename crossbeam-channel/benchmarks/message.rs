use std::fmt;

#[derive(Clone, Copy)]
pub struct Message(pub i32);

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad("Message")
    }
}

#[inline]
pub fn new(num: usize) -> Message {
    Message(num as i32)
}
