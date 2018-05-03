use std::cell::Cell;
use std::num::Wrapping;
use std::thread;

/// A counter that performs exponential backoff in spin loops.
pub struct Backoff(u32); // TODO: maybe it should use a Cell?

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
        if self.0 <= 6 {
            #[cfg(feature = "nightly")]
            for _ in 0..1 << self.0 {
                ::std::sync::atomic::spin_loop_hint();
            }
            self.0 += 1;
            true
        } else if self.0 <= 10 {
            // TODO: return false without yielding
            thread::yield_now();
            self.0 += 1;
            true
        } else {
            thread::yield_now();
            false
        }
    }
}

pub fn shuffle<T>(v: &mut [T]) {
    let len = v.len();
    if len <= 1 {
        return;
    }

    thread_local! {
        static RNG: Cell<Wrapping<u32>> = Cell::new(Wrapping(1));
    }

    RNG.with(|rng| {
        for i in 1..len {
            // This is the 32-bit variant of Xorshift.
            // https://en.wikipedia.org/wiki/Xorshift
            let mut x = rng.get();
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            rng.set(x);

            let x = x.0;
            let n = i + 1;

            // This is a fast alternative to `let j = x % n`.
            // https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
            let j = ((x as u64 * n as u64) >> 32) as u32 as usize;

            v.swap(i, j);
        }
    });
}
