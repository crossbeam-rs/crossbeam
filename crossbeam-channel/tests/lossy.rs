//! Tests for the lossy bounded channel.

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};

use crossbeam_channel::{RecvError, SendError, TryRecvError, TrySendError, lossy};
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

    assert!(s.try_send(1).is_ok());
    assert_eq!(s.try_send(2), Err(TrySendError::Full(1)));
    assert_eq!(s.try_send(3), Err(TrySendError::Full(2)));

    assert_eq!(r.recv(), Ok(3));
    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
#[should_panic(expected = "lossy channel capacity must be positive")]
fn zero_capacity_panics() {
    let _ = lossy::<i32>(0);
}

#[test]
fn len_empty_full() {
    let (s, r) = lossy(2);

    assert_eq!(s.len(), 0);
    assert!(s.is_empty());
    assert!(!s.is_full());

    s.try_send(()).unwrap();
    assert_eq!(s.len(), 1);
    assert!(!s.is_empty());
    assert!(!s.is_full());

    s.try_send(()).unwrap();
    assert_eq!(s.len(), 2);
    assert!(!s.is_empty());
    assert!(s.is_full());

    // Eviction keeps the channel full.
    let _ = s.try_send(());
    assert_eq!(s.len(), 2);
    assert!(s.is_full());

    r.recv().unwrap();
    assert_eq!(r.len(), 1);
    assert!(!r.is_full());
}

#[test]
fn try_send_evicts_oldest() {
    let (s, r) = lossy(2);

    assert!(s.try_send(1).is_ok());
    assert!(s.try_send(2).is_ok());

    // Full - evicts oldest (1).
    assert_eq!(s.try_send(3), Err(TrySendError::Full(1)));
    // Evicts 2.
    assert_eq!(s.try_send(4), Err(TrySendError::Full(2)));

    assert_eq!(r.recv(), Ok(3));
    assert_eq!(r.recv(), Ok(4));
    assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn try_send_no_eviction_when_space() {
    let (s, r) = lossy(3);

    assert!(s.try_send(1).is_ok());
    assert!(s.try_send(2).is_ok());
    assert!(s.try_send(3).is_ok());

    assert_eq!(r.recv(), Ok(1));

    // Space available now.
    assert!(s.try_send(4).is_ok());

    assert_eq!(r.recv(), Ok(2));
    assert_eq!(r.recv(), Ok(3));
    assert_eq!(r.recv(), Ok(4));
}

#[test]
fn send_blocks_when_full() {
    let (s, r) = lossy(1);
    s.send(1).unwrap();

    let received = AtomicUsize::new(0);
    scope(|scope| {
        scope.spawn(|_| {
            thread::sleep(Duration::from_millis(100));
            assert_eq!(r.recv(), Ok(1));
            received.store(1, Ordering::Release);
        });

        // If send evicted instead of blocking, it would return before the receiver runs.
        assert!(s.send(2).is_ok());
        assert_eq!(received.load(Ordering::Acquire), 1);
    })
    .unwrap();
}

#[test]
fn recv_after_disconnect() {
    let (s, r) = lossy(2);
    s.try_send(1).unwrap();
    s.try_send(2).unwrap();
    drop(s);

    assert_eq!(r.recv(), Ok(1));
    assert_eq!(r.recv(), Ok(2));
    assert_eq!(r.recv(), Err(RecvError));
}

#[test]
fn send_after_disconnect() {
    let (s, r) = lossy(100);

    s.try_send(1).unwrap();
    s.try_send(2).unwrap();
    drop(r);

    assert_eq!(s.send(3), Err(SendError(3)));
    assert_eq!(s.try_send(4), Err(TrySendError::Disconnected(4)));
}

#[test]
fn disconnect_wakes_sender() {
    let (s, r) = lossy(1);

    scope(|scope| {
        scope.spawn(move |_| {
            assert_eq!(s.send(()), Ok(()));
            assert_eq!(s.send(()), Err(SendError(())));
        });
        scope.spawn(move |_| {
            thread::sleep(Duration::from_millis(100));
            drop(r);
        });
    })
    .unwrap();
}

#[test]
fn try_send_disconnected() {
    let (s, r) = lossy(2);
    drop(r);

    assert_eq!(s.try_send(1), Err(TrySendError::Disconnected(1)));
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
    s.try_send(DropCounter).unwrap();
    s.try_send(DropCounter).unwrap();
    assert_eq!(DROPS.load(Ordering::SeqCst), 0);

    // Evict oldest - should drop it.
    let _ = s.try_send(DropCounter);
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);

    let _ = s.try_send(DropCounter);
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
                    let _ = s.try_send(i);
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
