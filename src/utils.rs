//! Miscellaneous utilities.

use std::cell::Cell;
use std::num::Wrapping;
use std::sync::atomic;
use std::thread;
use std::time::{Duration, Instant};

use rand;

/// A counter that performs exponential backoff in spin loops.
pub struct Backoff(u32);

impl Backoff {
    /// Creates a new `Backoff`.
    #[inline]
    pub fn new() -> Self {
        Backoff(0)
    }

    /// Backs off in a spin loop.
    ///
    /// This method may yield the current processor. Use it in lock-free retry loops.
    #[inline]
    pub fn spin(&mut self) {
        for _ in 0..1 << self.0.min(6) {
            atomic::spin_loop_hint();
        }
        self.0 = self.0.wrapping_add(1);
    }

    /// Backs off in a wait loop.
    ///
    /// Returns `true` if snoozing has reached a threshold where we should consider parking the
    /// thread instead.
    ///
    /// This method may yield the current processor or the current thread. Use it when waiting on a
    /// resource.
    #[inline]
    pub fn snooze(&mut self) -> bool {
        if self.0 <= 6 {
            for _ in 0..1 << self.0 {
                atomic::spin_loop_hint();
            }
        } else {
            thread::yield_now();
        }

        self.0 = self.0.wrapping_add(1);
        self.0 <= 10
    }
}

/// Randomly shuffles a slice.
pub fn shuffle<T>(v: &mut [T]) {
    let len = v.len();
    if len <= 1 {
        return;
    }

    thread_local! {
        static RNG: Cell<Wrapping<u32>> = {
            let init = rand::random::<u32>() | 1;
            Cell::new(Wrapping(init))
        }
    }

    let _ = RNG.try_with(|rng| {
        for i in 1..len {
            // This is the 32-bit variant of Xorshift.
            //
            // Source: https://en.wikipedia.org/wiki/Xorshift
            let mut x = rng.get();
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            rng.set(x);

            let x = x.0;
            let n = i + 1;

            // This is a fast alternative to `let j = x % n`.
            //
            // Author: Daniel Lemire
            // Source: https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
            let j = ((x as u64).wrapping_mul(n as u64) >> 32) as u32 as usize;

            v.swap(i, j);
        }
    });
}

/// Sleeps until the deadline, or forever if the deadline isn't specified.
pub fn sleep_until(deadline: Option<Instant>) {
    loop {
        match deadline {
            None => thread::sleep(Duration::from_secs(1000)),
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    break;
                }
                thread::sleep(d - now);
            }
        }
    }
}
