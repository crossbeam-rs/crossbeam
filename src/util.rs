use std::cell::Cell;
use std::num::Wrapping;
use std::thread;

/// A counter that performs exponential backoff in spin loops.
pub struct Backoff(u32);

impl Backoff {
    /// Returns a new `Backoff`.
    #[inline]
    pub fn new() -> Self {
        Backoff(0)
    }

    /// Increments the counter and backs off.
    ///
    /// Returns `true` if the counter has reached a large threshold. In that case it is advisable
    /// to break the loop, do something else, and try again later.
    ///
    /// This method may yield the current processor or the current thread.
    #[inline]
    pub fn step(&mut self) -> bool {
        if self.0 < 10 {
            #[cfg(feature = "nightly")]
            for _ in 0..1 << self.0 {
                ::std::sync::atomic::hint_core_should_pause();
            }
            self.0 += 1;
            true
        } else if self.0 < 20 {
            thread::yield_now();
            self.0 += 1;
            true
        } else {
            thread::yield_now();
            false
        }
    }
}

/// Returns a random number in range `0..ceil`.
pub fn small_random(ceil: usize) -> usize {
    thread_local! {
        static RNG: Cell<Wrapping<u32>> = Cell::new(Wrapping(1));
    }

    RNG.with(|rng| {
        // This is the 32-bit variant of Xorshift.
        let mut x = rng.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rng.set(x);

        // A modulo operation by a constant gets compiled to simple multiplication. The general
        // modulo instruction is in comparison much more expensive, so here we match on the most
        // common moduli in order to avoid it.
        let x = x.0 as usize;
        match ceil {
            1 => x % 1,
            2 => x % 2,
            3 => x % 3,
            4 => x % 4,
            5 => x % 5,
            n => x % n,
        }
    })
}
