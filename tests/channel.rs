extern crate crossbeam;
extern crate crossbeam_channel;

use crossbeam_channel::{bounded, unbounded};

#[test]
fn smoke() {
    unbounded::<i32>();
    bounded::<i32>(7);
    bounded::<i32>(0);
}

#[test]
fn nested_recv_iter() {
    let (tx, rx) = unbounded::<i32>();
    let (total_tx, total_rx) = unbounded::<i32>();

    crossbeam::scope(|s| {
        s.spawn(move || {
            let mut acc = 0;
            for x in rx.iter() {
                acc += x;
            }
            total_tx.send(acc);
        });

        tx.send(3);
        tx.send(1);
        tx.send(2);
        drop(tx);
        assert_eq!(total_rx.recv().unwrap(), 6);
    });
}

#[test]
fn recv_iter_break() {
    let (tx, rx) = unbounded::<i32>();
    let (count_tx, count_rx) = unbounded();

    crossbeam::scope(|s| {
        s.spawn(move || {
            let mut count = 0;
            for x in rx.iter() {
                if count >= 3 {
                    break;
                } else {
                    count += x;
                }
            }
            count_tx.send(count);
        });

        tx.send(2);
        tx.send(2);
        tx.send(2);
        let _ = tx.send(2);
        drop(tx);
        assert_eq!(count_rx.recv().unwrap(), 4);
    })
}

#[test]
fn recv_try_iter() {
    let (request_tx, request_rx) = unbounded();
    let (response_tx, response_rx) = unbounded();

    crossbeam::scope(|s| {
        // Request `x`s until we have `6`.
        s.spawn(move || {
            let mut count = 0;
            loop {
                for x in response_rx.try_iter() {
                    count += x;
                    if count == 6 {
                        assert_eq!(count, 6);
                        return;
                    }
                }
                request_tx.send(());
            }
        });

        for _ in request_rx.iter() {
            response_tx.send(2);
        }
    })
}

#[test]
fn recv_into_iter_owned() {
    let mut iter = {
        let (tx, rx) = unbounded::<i32>();
        tx.send(1);
        tx.send(2);
        rx.into_iter()
    };

    assert_eq!(iter.next().unwrap(), 1);
    assert_eq!(iter.next().unwrap(), 2);
    assert_eq!(iter.next().is_none(), true);
}

#[test]
fn recv_into_iter_borrowed() {
    let (tx, rx) = unbounded::<i32>();
    tx.send(1);
    tx.send(2);
    drop(tx);

    let mut iter = (&rx).into_iter();
    assert_eq!(iter.next().unwrap(), 1);
    assert_eq!(iter.next().unwrap(), 2);
    assert_eq!(iter.next().is_none(), true);
}
