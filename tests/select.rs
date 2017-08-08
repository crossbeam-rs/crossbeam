extern crate channel;
extern crate crossbeam;

use std::thread;
use std::time::Duration;

use channel::select;
use channel::{RecvError, SendError, RecvTimeoutError, SendTimeoutError, TryRecvError, TrySendError};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke1() {
    let (tx1, rx1) = channel::unbounded::<i32>();
    let (tx2, rx2) = channel::unbounded::<i32>();
    tx1.send(1).unwrap();

    loop {
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
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(v) = rx2.select() {
            assert_eq!(v, 2);
            break;
        }
    }
}

#[test]
fn disconnected() {
    let (tx1, rx1) = channel::unbounded::<i32>();
    let (tx2, rx2) = channel::unbounded::<i32>();

    drop(tx1);
    drop(tx2);

    loop {
        if let Ok(_) = rx1.select() {
            panic!();
        }
        if let Ok(_) = rx2.select() {
            panic!();
        }
        if select::disconnected() {
            break;
        }
    }
}

#[test]
fn select_recv() {
    let (tx1, rx1) = channel::bounded::<i32>(0);
    let (tx2, rx2) = channel::bounded::<i32>(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            // thread::sleep(ms(150));
            // tx1.send_timeout(1, ms(200));
            loop {
                match tx1.try_send(1) {
                    Ok(()) => break,
                    Err(TrySendError::Disconnected(_)) => break,
                    Err(TrySendError::Full(_)) => continue,
                }
            }
        });
        s.spawn(|| {
            // thread::sleep(ms(150));
            // tx2.send_timeout(2, ms(200));
            loop {
                match tx2.try_send(2) {
                    Ok(()) => break,
                    Err(TrySendError::Disconnected(_)) => break,
                    Err(TrySendError::Full(_)) => continue,
                }
            }
        });
        s.spawn(|| {
            thread::sleep(ms(100));
            loop {
                if let Ok(x) = rx1.select() {
                    println!("{}", x);
                    break;
                }
                if let Ok(x) = rx2.select() {
                    println!("{}", x);
                    break;
                }
                if select::disconnected() {
                    println!("DISCONNECTED!");
                    break;
                }
                if select::timeout(ms(100)) {
                    println!("TIMEOUT!");
                    break;
                }
            }
            drop(rx1);
            drop(rx2);
        });
    });
}

#[test]
fn select_send() {
    let (tx1, rx1) = channel::bounded::<i32>(0);
    let (tx2, rx2) = channel::bounded::<i32>(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(150));
            println!("got 1: {:?}", rx1.recv_timeout(ms(200)));
        });
        s.spawn(|| {
            thread::sleep(ms(150));
            println!("got 2: {:?}", rx2.recv_timeout(ms(200)));
        });
        s.spawn(|| {
            thread::sleep(ms(100));
            loop {
                if let Ok(()) = tx1.select(1) {
                    println!("sent 1");
                    break;
                }
                if let Ok(()) = tx2.select(2) {
                    println!("sent 2");
                    break;
                }
                if select::disconnected() {
                    println!("DISCONNECTED!");
                    break;
                }
                if select::timeout(ms(100)) {
                    println!("TIMEOUT!");
                    break;
                }
            }
        });
    });
}
