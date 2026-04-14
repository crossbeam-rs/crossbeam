//! Tests for the lossy bounded channel.

use std::sync::{
    Barrier,
    atomic::{AtomicUsize, Ordering},
};

use crossbeam_channel::{
    LossySendError, RecvError, SendError, TryRecvError, bounded, lossy, unbounded,
};
use crossbeam_utils::thread::scope;

#[test]
fn smoke() {
    let (s, r) = lossy(1);
    s.send(7).unwrap();
    assert_eq!(r.try_recv(), Ok(7));

    s.send(8).unwrap();
    assert_eq!(r.recv(), Ok(8));

    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn capacity() {
    for i in 1..10 {
        let (s, r) = lossy::<()>(i);
        assert_eq!(s.capacity(), Some(i));
        assert_eq!(r.capacity(), Some(i));
    }
}

#[test]
fn capacity_one() {
    let (s, r) = lossy(1);

    assert!(s.lossy_send(1).is_ok());
    assert_eq!(s.lossy_send(2), Err(LossySendError::Evicted(1)));
    assert_eq!(s.lossy_send(3), Err(LossySendError::Evicted(2)));

    assert_eq!(r.recv(), Ok(3));
    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
#[should_panic(expected = "capacity must be positive")]
fn zero_capacity_panics() {
    let _ = lossy::<i32>(0);
}

#[test]
fn len_empty_full() {
    let (s, r) = lossy(2);

    assert_eq!(s.len(), 0);
    assert!(s.is_empty());
    assert!(!s.is_full());

    s.lossy_send(()).unwrap();
    assert_eq!(s.len(), 1);
    assert!(!s.is_empty());
    assert!(!s.is_full());

    s.lossy_send(()).unwrap();
    assert_eq!(s.len(), 2);
    assert!(!s.is_empty());
    assert!(s.is_full());

    // Eviction keeps the channel full.
    let _ = s.lossy_send(());
    assert_eq!(s.len(), 2);
    assert!(s.is_full());

    r.recv().unwrap();
    assert_eq!(r.len(), 1);
    assert!(!r.is_full());
}

#[test]
fn lossy_send_evicts_oldest() {
    let (s, r) = lossy(2);

    assert!(s.lossy_send(1).is_ok());
    assert!(s.lossy_send(2).is_ok());

    // Full - evicts oldest (1).
    assert_eq!(s.lossy_send(3), Err(LossySendError::Evicted(1)));
    // Evicts 2.
    assert_eq!(s.lossy_send(4), Err(LossySendError::Evicted(2)));

    assert_eq!(r.recv(), Ok(3));
    assert_eq!(r.recv(), Ok(4));
    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn lossy_send_no_eviction_when_space() {
    let (s, r) = lossy(3);

    assert!(s.lossy_send(1).is_ok());
    assert!(s.lossy_send(2).is_ok());
    assert!(s.lossy_send(3).is_ok());

    assert_eq!(r.recv(), Ok(1));

    // Space available now.
    assert!(s.lossy_send(4).is_ok());

    assert_eq!(r.recv(), Ok(2));
    assert_eq!(r.recv(), Ok(3));
    assert_eq!(r.recv(), Ok(4));
}

#[test]
fn send_blocks_when_full() {
    let (s, r) = lossy(1);
    s.send(1).unwrap();

    let barrier = Barrier::new(2);
    let received = AtomicUsize::new(0);
    scope(|scope| {
        scope.spawn(|_| {
            assert_eq!(r.recv(), Ok(1));
            received.store(1, Ordering::Release);
            barrier.wait();
        });

        // If send evicted instead of blocking, it would return before the receiver runs.
        s.send(2).unwrap();
        barrier.wait();
        assert_eq!(received.load(Ordering::Acquire), 1);
    })
    .unwrap();
}

#[test]
fn recv_after_disconnect() {
    let (s, r) = lossy(2);
    s.lossy_send(1).unwrap();
    s.lossy_send(2).unwrap();
    drop(s);

    assert_eq!(r.recv(), Ok(1));
    assert_eq!(r.recv(), Ok(2));
    assert_eq!(r.recv(), Err(RecvError));
}

#[test]
fn send_after_disconnect() {
    let (s, r) = lossy(100);

    s.lossy_send(1).unwrap();
    s.lossy_send(2).unwrap();
    drop(r);

    assert_eq!(s.send(3), Err(SendError(3)));
    assert_eq!(s.lossy_send(4), Err(LossySendError::Disconnected(4)));
}

#[test]
fn disconnect_wakes_sender() {
    let (s, r) = lossy(1);
    let barrier = Barrier::new(2);

    scope(|scope| {
        scope.spawn(|_| {
            assert_eq!(s.send(()), Ok(()));
            barrier.wait();
            assert_eq!(s.send(()), Err(SendError(())));
        });
        scope.spawn(|_| {
            barrier.wait();
            drop(r);
        });
    })
    .unwrap();
}

#[test]
fn lossy_send_disconnected() {
    let (s, r) = lossy(2);
    drop(r);

    assert_eq!(s.lossy_send(1), Err(LossySendError::Disconnected(1)));
}

#[test]
fn drops() {
    static DROPS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Debug, PartialEq)]
    struct DropCounter;

    impl Drop for DropCounter {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    DROPS.store(0, Ordering::SeqCst);
    let (s, r) = lossy(2);
    s.lossy_send(DropCounter).unwrap();
    s.lossy_send(DropCounter).unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);

    // Evict oldest - should drop it.
    let _ = s.lossy_send(DropCounter);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);

    let _ = s.lossy_send(DropCounter);
    assert_eq!(DROPS.load(Ordering::SeqCst), 2);

    // Remaining items dropped when channel is dropped.
    drop(s);
    drop(r);
    assert_eq!(DROPS.load(Ordering::SeqCst), 4);
}

#[test]
fn spsc() {
    const COUNT: usize = if cfg!(miri) { 100 } else { 100_000 };

    let (s, r) = lossy(3);

    scope(|scope| {
        scope.spawn(move |_| {
            for i in 0..COUNT {
                assert_eq!(r.recv(), Ok(i));
            }
            assert_eq!(r.recv(), Err(RecvError));
        });
        scope.spawn(move |_| {
            for i in 0..COUNT {
                s.send(i).unwrap();
            }
        });
    })
    .unwrap();
}

#[test]
fn mpmc_lossy() {
    let (s, r) = lossy(4);
    let total = AtomicUsize::new(0);

    scope(|scope| {
        // Two producers sending 100 each.
        for _ in 0..2 {
            let s = s.clone();
            scope.spawn(move |_| {
                for i in 0..100 {
                    let _ = s.lossy_send(i);
                }
            });
        }
        drop(s);

        // Two consumers.
        let total = &total;
        for _ in 0..2 {
            let r = r.clone();
            scope.spawn(move |_| {
                while r.recv().is_ok() {
                    total.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    })
    .unwrap();

    // Some messages may have been evicted, but we should have received some.
    assert!(total.load(Ordering::Relaxed) > 0);
}

// Tests for lossy_send on non-lossy channels.

#[test]
fn lossy_send_on_unbounded() {
    let (s, r) = unbounded();

    assert!(s.lossy_send(1).is_ok());
    assert!(s.lossy_send(2).is_ok());
    assert!(s.lossy_send(3).is_ok());

    assert_eq!(r.recv(), Ok(1));
    assert_eq!(r.recv(), Ok(2));
    assert_eq!(r.recv(), Ok(3));

    drop(r);
    assert_eq!(s.lossy_send(4), Err(LossySendError::Disconnected(4)));
}

#[test]
fn lossy_send_on_zero_capacity() {
    let (s, r) = bounded::<i32>(0);

    // No receiver waiting — NotSent.
    assert_eq!(s.lossy_send(1), Err(LossySendError::NotSent(1)));

    drop(r);
    assert_eq!(s.lossy_send(2), Err(LossySendError::Disconnected(2)));

    // The success path (receiver already waiting) is not tested here because there is no
    // simple reliable way to guarantee the receiver is blocked in recv() before lossy_send
    // fires. That path delegates to zero::try_send internally, which is covered by the
    // zero-capacity channel tests.
}
