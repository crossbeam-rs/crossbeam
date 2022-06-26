use crossbeam_deque::Steal::Success;
use crossbeam_deque::{Injector, Worker};

#[cfg(all(test, crossbeam_loom))]
use loom_crate as loom;
#[cfg(all(test, not(crossbeam_loom)))]
mod loom {
    pub fn model(mut fun: impl FnMut()) {
        fun();
    }
}

#[test]
fn steal_fifo() {
    loom::model(|| {
        let w = Worker::new_fifo();
        for i in 1..=3 {
            w.push(i);
        }

        let s = w.stealer();
        assert_eq!(s.steal(), Success(1));
        assert_eq!(s.steal(), Success(2));
        assert_eq!(s.steal(), Success(3));
    });
}

#[test]
fn steal_lifo() {
    loom::model(|| {
        let w = Worker::new_lifo();
        for i in 1..=3 {
            w.push(i);
        }

        let s = w.stealer();
        assert_eq!(s.steal(), Success(1));
        assert_eq!(s.steal(), Success(2));
        assert_eq!(s.steal(), Success(3));
    });
}

#[test]
fn steal_injector() {
    loom::model(|| {
        let q = Injector::new();
        for i in 1..=3 {
            q.push(i);
        }

        assert_eq!(q.steal(), Success(1));
        assert_eq!(q.steal(), Success(2));
        assert_eq!(q.steal(), Success(3));
    });
}

#[test]
fn steal_batch_fifo_fifo() {
    loom::model(|| {
        let w = Worker::new_fifo();
        for i in 1..=4 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_fifo();

        assert_eq!(s.steal_batch(&w2), Success(()));
        assert_eq!(w2.pop(), Some(1));
        assert_eq!(w2.pop(), Some(2));
    });
}

#[test]
fn steal_batch_lifo_lifo() {
    loom::model(|| {
        let w = Worker::new_lifo();
        for i in 1..=4 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_lifo();

        assert_eq!(s.steal_batch(&w2), Success(()));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(1));
    });
}

#[test]
fn steal_batch_fifo_lifo() {
    loom::model(|| {
        let w = Worker::new_fifo();
        for i in 1..=4 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_lifo();

        assert_eq!(s.steal_batch(&w2), Success(()));
        assert_eq!(w2.pop(), Some(1));
        assert_eq!(w2.pop(), Some(2));
    });
}

#[test]
fn steal_batch_lifo_fifo() {
    loom::model(|| {
        let w = Worker::new_lifo();
        for i in 1..=4 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_fifo();

        assert_eq!(s.steal_batch(&w2), Success(()));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(1));
    });
}

#[test]
fn steal_batch_injector_fifo() {
    loom::model(|| {
        let q = Injector::new();
        for i in 1..=4 {
            q.push(i);
        }

        let w2 = Worker::new_fifo();
        assert_eq!(q.steal_batch(&w2), Success(()));
        assert_eq!(w2.pop(), Some(1));
        assert_eq!(w2.pop(), Some(2));
    });
}

#[test]
fn steal_batch_injector_lifo() {
    loom::model(|| {
        let q = Injector::new();
        for i in 1..=4 {
            q.push(i);
        }

        let w2 = Worker::new_lifo();
        assert_eq!(q.steal_batch(&w2), Success(()));
        assert_eq!(w2.pop(), Some(1));
        assert_eq!(w2.pop(), Some(2));
    });
}

#[test]
fn steal_batch_and_pop_fifo_fifo() {
    loom::model(|| {
        let w = Worker::new_fifo();
        for i in 1..=6 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_fifo();

        assert_eq!(s.steal_batch_and_pop(&w2), Success(1));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(3));
    });
}

#[test]
fn steal_batch_and_pop_lifo_lifo() {
    loom::model(|| {
        let w = Worker::new_lifo();
        for i in 1..=6 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_lifo();

        assert_eq!(s.steal_batch_and_pop(&w2), Success(3));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(1));
    });
}

#[test]
fn steal_batch_and_pop_fifo_lifo() {
    loom::model(|| {
        let w = Worker::new_fifo();
        for i in 1..=6 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_lifo();

        assert_eq!(s.steal_batch_and_pop(&w2), Success(1));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(3));
    });
}

#[test]
fn steal_batch_and_pop_lifo_fifo() {
    loom::model(|| {
        let w = Worker::new_lifo();
        for i in 1..=6 {
            w.push(i);
        }

        let s = w.stealer();
        let w2 = Worker::new_fifo();

        assert_eq!(s.steal_batch_and_pop(&w2), Success(3));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(1));
    });
}

#[test]
fn steal_batch_and_pop_injector_fifo() {
    loom::model(|| {
        let q = Injector::new();
        for i in 1..=6 {
            q.push(i);
        }

        let w2 = Worker::new_fifo();
        assert_eq!(q.steal_batch_and_pop(&w2), Success(1));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(3));
    });
}

#[test]
fn steal_batch_and_pop_injector_lifo() {
    loom::model(|| {
        let q = Injector::new();
        for i in 1..=6 {
            q.push(i);
        }

        let w2 = Worker::new_lifo();
        assert_eq!(q.steal_batch_and_pop(&w2), Success(1));
        assert_eq!(w2.pop(), Some(2));
        assert_eq!(w2.pop(), Some(3));
    });
}
