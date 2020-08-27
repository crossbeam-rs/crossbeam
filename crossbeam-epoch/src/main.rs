use core::sync::atomic::Ordering;
use crossbeam_epoch::arc::{Arc, Atomic};
use crossbeam_epoch::pin;

fn main() {
    let a = Atomic::<Arc<usize>>::new(Arc::new(123));

    let n = 10_000_000;

    for _ in 0..n {
        let guard = pin();
        unsafe { a.load(Ordering::Acquire, &guard) };
    }
}
