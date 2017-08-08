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
            if self.0 < 2 {
                ()
            } else if self.0 < 10 {
                for _ in 0 .. 1 << self.0 {
                    ::std::sync::atomic::hint_core_should_pause();
                }
            } else {
                // TODO: probably shouldn't yield when pinned - parking_lot has two backoff
                // variants. also, try parking_lot in nightly mode
                thread::yield_now();
            }

            self.0 += 1;
            true
        }
    }
}
