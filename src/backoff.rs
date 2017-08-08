use std::sync::atomic;
use std::thread;

#[derive(Clone, Copy)]
pub struct Backoff(usize);

impl Backoff {
    #[inline]
    pub fn new() -> Self {
        Backoff(0)
    }

    #[inline]
    pub fn tick(&mut self) -> bool {
        if self.0 >= 20 {
            false
        } else {
            if self.0 >= 10 {
                thread::yield_now();
            }

            self.0 += 1;
            true
        }
    }
}
