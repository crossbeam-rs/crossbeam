use crossbeam_channel::{watch, RecvError, TryRecvError};
use std::thread;
use std::time::Duration;

#[test]
fn watch_send_recv_basic() {
    let (tx, mut rx) = watch::<i32>();
    tx.send(42).unwrap();
    assert_eq!(rx.recv(), Ok(42));
}

#[test]
fn watch_overwrites_value() {
    let (tx, mut rx) = watch::<i32>();
    tx.send(1).unwrap();
    tx.send(2).unwrap();
    tx.send(3).unwrap();
    // Only the latest value should be received.
    assert_eq!(rx.recv(), Ok(3));
}

#[test]
fn watch_recv_blocks_until_send() {
    let (tx, mut rx) = watch::<i32>();

    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        tx.send(99).unwrap();
    });

    assert_eq!(rx.recv(), Ok(99));
    handle.join().unwrap();
}

#[test]
fn watch_recv_error_on_disconnect() {
    let (tx, mut rx) = watch::<i32>();
    drop(tx);
    assert_eq!(rx.recv(), Err(RecvError));
}

#[test]
fn watch_try_recv_empty() {
    let (_tx, mut rx) = watch::<i32>();
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn watch_try_recv_disconnected() {
    let (tx, mut rx) = watch::<i32>();
    drop(tx);
    assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
}

#[test]
fn watch_try_recv_value() {
    let (tx, mut rx) = watch::<i32>();
    tx.send(10).unwrap();
    assert_eq!(rx.try_recv(), Ok(10));
    // Second try_recv should be empty since version hasn't changed.
    assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn watch_send_error_when_no_receivers() {
    let (tx, rx) = watch::<i32>();
    drop(rx);
    assert!(tx.send(1).is_err());
}

#[test]
fn watch_multiple_sends_between_recvs() {
    let (tx, mut rx) = watch::<i32>();
    tx.send(1).unwrap();
    assert_eq!(rx.recv(), Ok(1));

    tx.send(2).unwrap();
    tx.send(3).unwrap();
    // Should only get the latest.
    assert_eq!(rx.recv(), Ok(3));
}

#[test]
fn watch_clone_sender() {
    let (tx1, mut rx) = watch::<i32>();
    let tx2 = tx1.clone();

    tx1.send(1).unwrap();
    assert_eq!(rx.recv(), Ok(1));

    tx2.send(2).unwrap();
    assert_eq!(rx.recv(), Ok(2));

    drop(tx1);
    // Channel still alive because tx2 exists.
    tx2.send(3).unwrap();
    assert_eq!(rx.recv(), Ok(3));

    drop(tx2);
    assert_eq!(rx.recv(), Err(RecvError));
}

#[test]
fn watch_clone_receiver() {
    let (tx, mut rx1) = watch::<i32>();
    let mut rx2 = rx1.clone();

    tx.send(1).unwrap();
    // Both receivers should be able to receive the value independently.
    assert_eq!(rx1.recv(), Ok(1));
    assert_eq!(rx2.recv(), Ok(1));

    tx.send(2).unwrap();
    assert_eq!(rx1.recv(), Ok(2));
    assert_eq!(rx2.recv(), Ok(2));
}

#[test]
fn watch_multithreaded() {
    let (tx, mut rx) = watch::<i32>();

    let producer = thread::spawn(move || {
        for i in 0..100 {
            tx.send(i).unwrap();
        }
    });

    // The receiver should eventually see 99 (or possibly skip some values).
    let mut last_seen = -1;
    loop {
        match rx.recv() {
            Ok(v) => {
                assert!(v > last_seen || v == 0);
                last_seen = v;
                if v == 99 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    assert_eq!(last_seen, 99);
    producer.join().unwrap();
}
