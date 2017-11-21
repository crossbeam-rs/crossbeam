use std::{mem, cell};

#[derive(Debug)]
pub struct Spot<T>(cell::RefCell<Inner<T>>);

#[derive(Debug)]
enum Inner<T> {
    Present(T),
    Empty,
}

impl<T> Spot<T> {
    pub fn new(t: T) -> Self {
        Spot(cell::RefCell::new(Inner::Present(t)))
    }

    pub fn take(&self) -> T {
        self.0.borrow_mut().take()
    }
}

impl<T> Inner<T> {
    pub fn unpack(self) -> T {
        match self {
            Inner::Present(t) => t,
            Inner::Empty => unreachable!(),
        }
    }

    pub fn take(&mut self) -> T {
        mem::replace(self, Inner::Empty).unpack()
    }
}
