extern crate channel;
extern crate crossbeam;

use std::thread;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use channel::{bounded, select, unbounded};
use channel::{RecvError, RecvTimeoutError, SendError, SendTimeoutError, TryRecvError, TrySendError};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

// TODO: clone rx/tx, then drop and check disconnected

#[test]
fn smoke1() {
    let mut iters = 0;
    let (tx1, rx1) = unbounded();
    let (tx2, rx2) = unbounded();

    tx1.send(1).unwrap();
    loop {
        iters += 1;
        if let Ok(v) = rx1.select() {
            assert_eq!(v, 1);
            break;
        }
        if let Ok(_) = rx2.select() {
            panic!();
        }
    }

    tx2.send(2).unwrap();
    loop {
        iters += 1;
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(v) = rx2.select() {
            assert_eq!(v, 2);
            break;
        }
    }

    assert!(iters < 50);
}

#[test]
fn smoke2() {
    let mut iters = 0;
    let (_tx1, rx1) = unbounded::<i32>();
    let (_tx2, rx2) = unbounded::<i32>();
    let (_tx3, rx3) = unbounded::<i32>();
    let (_tx4, rx4) = unbounded::<i32>();
    let (tx5, rx5) = unbounded::<i32>();

    tx5.send(5).unwrap();

    loop {
        iters += 1;
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(_) = rx2.select() {
            panic!();
        }
        if let Ok(_) = rx3.select() {
            panic!();
        }
        if let Ok(_) = rx4.select() {
            panic!();
        }
        if let Ok(x) = rx5.select() {
            assert_eq!(x, 5);
            break;
        }
    }

    assert!(iters < 50);
}

#[test]
fn disconnected() {
    let mut iters = 0;
    let (tx1, rx1) = unbounded::<i32>();
    let (tx2, rx2) = unbounded::<i32>();

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(50));
            drop(tx1);
        });

        loop {
            iters += 1;
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(_) = rx2.select() {
                panic!();
            }
            if select::disconnected() {
                panic!();
            }
            if select::timeout(ms(100)) {
                break;
            }
        }
    });

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(50));
            drop(tx2);
        });

        loop {
            iters += 1;
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(_) = rx2.select() {
                panic!();
            }
            if select::disconnected() {
                break;
            }
            if select::timeout(ms(100)) {
                panic!();
            }
        }
    });

    assert!(iters < 50);
}

#[test]
fn blocked() {
    let mut iters = 0;
    let (tx1, rx1) = unbounded::<i32>();
    let (tx2, rx2) = unbounded::<i32>();

    drop(tx1);
    loop {
        iters += 1;
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(_) = rx2.select() {
            panic!();
        }
        if select::disconnected() {
            panic!();
        }
        if select::blocked() {
            break;
        }
        if select::timeout(ms(0)) {
            panic!();
        }
    }

    tx2.send(2);
    loop {
        iters += 1;
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(x) = rx2.select() {
            assert_eq!(x, 2);
            break;
        }
        if select::disconnected() {
            panic!();
        }
        if select::blocked() {
            panic!();
        }
        if select::timeout(ms(0)) {
            panic!();
        }
    }

    drop(tx2);
    loop {
        iters += 1;
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(x) = rx2.select() {
            panic!();
        }
        if select::disconnected() {
            break;
        }
        if select::blocked() {
            panic!();
        }
        if select::timeout(ms(0)) {
            panic!();
        }
    }

    assert!(iters < 50);
}

#[test]
fn timeout() {
    let mut iters = 0;
    let (tx1, rx1) = unbounded::<i32>();
    let (tx2, rx2) = unbounded::<i32>();

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(150));
            tx2.send(2);
        });

        loop {
            iters += 1;
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(_) = rx2.select() {
                panic!();
            }
            if select::timeout(ms(100)) {
                break;
            }
        }

        loop {
            iters += 1;
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(x) = rx2.select() {
                assert_eq!(x, 2);
                break;
            }
            if select::timeout(ms(100)) {
                panic!();
            }
        }
    });

    assert!(iters < 50);
}

#[test]
fn unblocks() {
    let mut iters = 0;
    let (tx1, rx1) = bounded(0);
    let (tx2, rx2) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(50));
            tx2.send(2);
        });

        loop {
            iters += 1;
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(x) = rx2.select() {
                assert_eq!(x, 2);
                break;
            }
            if select::timeout(ms(100)) {
                panic!();
            }
        }
    });

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(50));
            assert_eq!(rx1.recv().unwrap(), 1);
        });

        loop {
            iters += 1;
            if let Ok(()) = tx1.select(1) {
                break;
            }
            if let Ok(()) = tx2.select(2) {
                panic!();
            }
            if select::timeout(ms(100)) {
                panic!();
            }
        }
    });

    assert!(iters < 50);
}

#[test]
fn both_ready() {
    let mut iters = 0;
    let (tx1, rx1) = bounded(0);
    let (tx2, rx2) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(50));
            tx1.send(1).unwrap();
            assert_eq!(rx2.recv().unwrap(), 2);
        });

        for _ in 0..2 {
            loop {
                iters += 1;
                if let Ok(x) = rx1.select() {
                    assert_eq!(x, 1);
                    break;
                }
                if let Ok(()) = tx2.select(2) {
                    break;
                }
            }
        }
    });

    assert!(iters < 50);
}

#[test]
fn no_starvation() {
    const N: usize = 10;

    let done_rx = &(0..N).map(|_| AtomicBool::new(false)).collect::<Vec<_>>();
    let done_tx = &(0..N).map(|_| AtomicBool::new(false)).collect::<Vec<_>>();

    while !done_rx.iter().all(|x| x.load(SeqCst)) || !done_tx.iter().all(|x| x.load(SeqCst)) {
        crossbeam::scope(|s| {
            let rxs = (0..N).map(|i| {
                let (tx, rx) = unbounded();
                tx.send(i).unwrap();
                rx
            }).collect::<Vec<_>>();

            let txs = (0..N).map(|i| {
                let (tx, rx) = bounded(0);
                s.spawn(move || {
                    if let Ok(x) = rx.recv() {
                        assert_eq!(x, i);
                        done_tx[i].store(true, SeqCst);
                    }
                });
                tx
            }).collect::<Vec<_>>();

            let mut iters = 0;

            'select: loop {
                iters += 1;

                for rx in &rxs {
                    if let Ok(x) = rx.select() {
                        done_rx[x].store(true, SeqCst);
                        break 'select;
                    }
                }

                for (i, tx) in txs.iter().enumerate() {
                    if let Ok(()) = tx.select(i) {
                        break 'select;
                    }
                }
            }

            assert!(iters < 50);
        });
    }
}

#[test]
fn loop_try() {
    for _ in 0..20 {
        let (tx1, rx1) = bounded::<i32>(0);
        let (tx2, rx2) = bounded::<i32>(0);

        crossbeam::scope(|s| {
            s.spawn(|| {
                loop {
                    match tx1.try_send(1) {
                        Ok(()) => break,
                        Err(TrySendError::Disconnected(_)) => break,
                        Err(TrySendError::Full(_)) => continue,
                    }
                }
            });

            s.spawn(|| {
                loop {
                    match rx2.try_recv() {
                        Ok(x) => {
                            assert_eq!(x, 2);
                            break;
                        }
                        Err(TryRecvError::Disconnected) => break,
                        Err(TryRecvError::Empty) => continue,
                    }
                }
            });

            s.spawn(|| {
                let mut iters = 0;
                thread::sleep(ms(50));

                loop {
                    iters += 1;
                    if let Ok(x) = rx1.select() {
                        assert_eq!(x, 1);
                        break;
                    }
                    if let Ok(x) = tx2.select(2) {
                        break;
                    }
                    if select::disconnected() {
                        panic!();
                    }
                    if select::timeout(ms(50)) {
                        panic!();
                    }
                }

                drop(rx1);
                drop(tx2);
                assert!(iters < 50);
            });
        });
    }
}

#[test]
fn cloning1() {
    crossbeam::scope(|s| {
        let mut iters = 0;
        let (tx1, rx1) = unbounded::<i32>();
        let (_tx2, rx2) = unbounded::<i32>();
        let (tx3, rx3) = unbounded::<()>();

        s.spawn(move || {
            rx3.recv().unwrap();
            tx1.clone();
            assert_eq!(rx3.try_recv(), Err(TryRecvError::Empty));
            tx1.send(1).unwrap();
            rx3.recv().unwrap();
        });

        tx3.send(()).unwrap();
        loop {
            iters += 1;
            if let Ok(_) = rx1.select() {
                break;
            }
            if let Ok(_) = rx2.select() {
                panic!();
            }
        }

        tx3.send(()).unwrap();
        assert!(iters < 50);
    });
}

#[test]
fn cloning2() {
    crossbeam::scope(|s| {
        let mut iters = 0;
        let (tx1, rx1) = unbounded::<()>();
        let (tx2, rx2) = unbounded::<()>();
        let (tx3, rx3) = unbounded::<()>();

        s.spawn(move || {
            loop {
                iters += 1;
                if let Ok(_) = rx1.select() {
                    panic!();
                }
                if let Ok(_) = rx2.select() {
                    break;
                }
            }
        });

        thread::sleep(ms(50));
        drop(tx1.clone());
        tx2.send(()).unwrap();

        assert!(iters < 50);
    })
}

#[test]
fn preflight1() {
    let (tx, rx) = unbounded();
    tx.send(()).unwrap();

    let mut iters = 0;
    loop {
        iters += 1;
        if let Ok(_) = rx.select() {
            break;
        }
    }
    assert!(iters < 10);
}

#[test]
fn preflight2() {
    let (tx, rx) = unbounded();
    drop(tx.clone());
    tx.send(()).unwrap();
    drop(tx);

    let mut iters = 0;
    loop {
        iters += 1;
        if let Ok(_) = rx.select() {
            break;
        }
    }
    assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    assert!(iters < 10);
}

#[test]
fn preflight3() {
    let (tx, rx) = unbounded();
    drop(tx.clone());
    tx.send(()).unwrap();
    drop(tx);
    rx.recv().unwrap();

    let mut iters = 0;
    loop {
        iters += 1;
        if let Ok(_) = rx.select() {
            panic!();
        }
        if select::disconnected() {
            break;
        }
    }
    assert!(iters < 10);
}

#[test]
fn stress_recv() {
    let (tx1, rx1) = unbounded();
    let (tx2, rx2) = bounded(5);
    let (tx3, rx3) = bounded(100);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..10_000 {
                tx1.send(i);
                rx3.recv().unwrap();

                tx2.send(i);
                rx3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            let mut iters = 0;

            for _ in 0..2 {
                loop {
                    iters += 1;
                    if let Ok(x) = rx1.select() {
                        assert_eq!(x, i);
                        break;
                    }
                    if let Ok(x) = rx2.select() {
                        assert_eq!(x, i);
                        break;
                    }
                }

                tx3.send(()).unwrap();
            }

            assert!(iters < 50);
        }
    });
}

#[test]
fn stress_send() {
    let (tx1, rx1) = bounded(0);
    let (tx2, rx2) = bounded(0);
    let (tx3, rx3) = bounded(100);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..10_000 {
                assert_eq!(rx1.recv().unwrap(), i);
                assert_eq!(rx2.recv().unwrap(), i);
                rx3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            let mut iters = 0;

            for _ in 0..2 {
                loop {
                    iters += 1;
                    if let Ok(()) = tx1.select(i) {
                        break;
                    }
                    if let Ok(()) = tx2.select(i) {
                        break;
                    }
                }
            }
            tx3.send(()).unwrap();

            assert!(iters < 50);
        }
    });
}

#[test]
fn stress_mixed() {
    let (tx1, rx1) = bounded(0);
    let (tx2, rx2) = bounded(0);
    let (tx3, rx3) = bounded(100);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..10_000 {
                tx1.send(i).unwrap();
                assert_eq!(rx2.recv().unwrap(), i);
                rx3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            let mut iters = 0;

            for _ in 0..2 {
                loop {
                    iters += 1;
                    if let Ok(x) = rx1.select() {
                        assert_eq!(x, i);
                        break;
                    }
                    if let Ok(()) = tx2.select(i) {
                        break;
                    }
                }
            }
            tx3.send(()).unwrap();

            assert!(iters < 50);
        }
    });
}
