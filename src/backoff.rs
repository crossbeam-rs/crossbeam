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
