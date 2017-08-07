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
    pub fn abort(&mut self) {
        self.0 = !0;
    }

    #[inline]
    pub fn tick(&mut self) -> bool {
        if self.0 >= 20 {
            false
        } else {
            self.0 += 1;

            if self.0 <= 10 {
                for _ in 0 .. 4 << self.0 {
                    ::std::sync::atomic::hint_core_should_pause();
                }
            } else {
                thread::yield_now();
            }

            true
        }
    }
}
