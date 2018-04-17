extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;

use std::any::Any;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

// TODO: test that `select!` evaluates to an expression
// TODO: two nested `select!`s
// TODO: check `select! { recv(&&&&&rx, _) => {} }`

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn foo() {
    let (s, r) = bounded::<i32>(5);

    crossbeam::scope(|scope| {
        // scope.spawn(|| {
        //     assert_eq!(r.recv(), None);
        //     assert_eq!(r.len(), 0);
        // });

        use std::panic;
        panic::catch_unwind(panic::AssertUnwindSafe(|| {
            select! {
                send(s, -i32::min_value()) => {}
            }
        }));
        assert_eq!(s.len(), 0);
        drop(s);
    });
}

// #[test]
// fn bar() {
//     let (tx, rx) = bounded(0);
//
//     crossbeam::scope(|s| {
//         s.spawn(|| {
//             tx.send(8);
//         });
//         s.spawn(|| {
//             thread::sleep(ms(1000)); /////////////
//             select! {
//                 recv(rx, v) => assert_eq!(v, Some(8))
//             }
//         });
//     });
// }

// #[test]
// #[allow(dead_code, unused_mut)]
// fn it_compiles() {
//     struct Foo(String);
//
//     fn foo(
//         mut struct_val: Foo,
//         mut var: String,
//         immutable_var: String,
//         rx0: Receiver<String>,
//         rx1: &Receiver<u32>,
//         rx2: Receiver<()>,
//         tx0: &mut Sender<String>,
//         tx1: Sender<String>,
//         tx2: Sender<String>,
//         tx3: Sender<String>,
//         tx4: Sender<String>,
//         tx5: Sender<u32>,
//     ) -> Option<String> {
//         select! {
//             recv(rx0, val) => Some(val),
//             recv(rx1, val) => Some(val.to_string()),
//             recv(rx2, ()) => None,
//             send(tx0, mut struct_val.0) => Some(var),
//             send(tx1, mut var) => Some(struct_val.0),
//             send(tx1, immutable_var) => Some(struct_val.0),
//             send(tx2, struct_val.0.clone()) => Some(struct_val.0),
//             send(tx3, "foo".to_string()) => Some(var),
//             send(tx4, var.clone()) => Some(var),
//             send(tx5, 42) => None,
//
//             closed() => Some("closed".into()),
//             would_block() => Some("would_block".into()),
//             timed_out(Duration::from_secs(1)) => Some("timed_out".into()),
//             // The previous timeout duration is overridden.
//             timed_out(Duration::from_secs(2)) => Some("timed_out".into()),
//         }
//     }
// }

#[test]
fn smoke1() {
    let (tx1, rx1) = unbounded();
    let (tx2, rx2) = unbounded();

    tx1.send(1);

    select! {
        recv(rx1, v) => assert_eq!(v, Some(1)),
        recv(rx2, _) => panic!(),
    }

    tx2.send(2);

    select! {
        recv(rx1, _) => panic!(),
        recv(rx2, v) => assert_eq!(v, Some(2)),
    }
}

#[test]
fn smoke2() {
    let (_tx1, rx1) = unbounded::<i32>();
    let (_tx2, rx2) = unbounded::<i32>();
    let (_tx3, rx3) = unbounded::<i32>();
    let (_tx4, rx4) = unbounded::<i32>();
    let (tx5, rx5) = unbounded::<i32>();

    tx5.send(5);

    select! {
        recv(rx1, _) => panic!(),
        recv(rx2, _) => panic!(),
        recv(rx3, _) => panic!(),
        recv(rx4, _) => panic!(),
        recv(rx5, v) => assert_eq!(v, Some(5)),
    }
}

#[test]
fn closed() {
    let (tx1, rx1) = unbounded::<i32>();
    let (tx2, rx2) = unbounded::<i32>();

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(500));
            drop(tx1);
            tx2.send(5);
        });

        select! {
            recv(rx1, v) => assert!(v.is_none()),
            recv(rx2, _) => panic!(),
            default(ms(1000)) => panic!(),
        }

        rx2.recv().unwrap();
    });

    select! {
        recv(rx1, v) => assert!(v.is_none()),
        recv(rx2, _) => panic!(),
        default(ms(1000)) => panic!(),
    }

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(500));
            drop(tx2);
        });

        select! {
            recv(rx2, v) => assert!(v.is_none()),
            default(ms(1000)) => panic!(),
        }
    });
}

#[test]
fn default() {
    let (tx1, rx1) = unbounded::<i32>();
    let (tx2, rx2) = unbounded::<i32>();

    select! {
        recv(rx1, _) => panic!(),
        recv(rx2, _) => panic!(),
        default => {}
    }

    drop(tx1);

    select! {
        recv(rx1, v) => assert!(v.is_none()),
        recv(rx2, _) => panic!(),
        default => panic!(),
    }

    tx2.send(2);

    select! {
        recv(rx2, v) => assert_eq!(v, Some(2)),
        default => panic!(),
    }

    select! {
        recv(rx2, _) => panic!(),
        default => {},
    }

    select! {
        default => {},
    }
}

#[test]
fn timeout() {
    let (_tx1, rx1) = unbounded::<i32>();
    let (tx2, rx2) = unbounded::<i32>();

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(1500));
            tx2.send(2);
        });

        select! {
            recv(rx1, _) => panic!(),
            recv(rx2, _) => panic!(),
            default(ms(1000)) => {},
        }

        select! {
            recv(rx1, _) => panic!(),
            recv(rx2, v) => assert_eq!(v, Some(2)),
            default(ms(1000)) => panic!(),
        }
    });

    crossbeam::scope(|s| {
        let (tx, rx) = unbounded::<i32>();

        s.spawn(move || {
            thread::sleep(ms(500));
            drop(tx);
        });

        select! {
            default(ms(1000)) => {
                select! {
                    recv(rx, v) => assert!(v.is_none()),
                    default => panic!(),
                }
            }
        }
    });
}

#[test]
fn default_when_closed() {
    let (_, rx) = unbounded::<i32>();

    select! {
        recv(rx, v) => assert!(v.is_none()),
        default => panic!(),
    }

    let (_, rx) = unbounded::<i32>();

    select! {
        recv(rx, v) => assert!(v.is_none()),
        default(ms(1000)) => panic!(),
    }
}

#[test]
fn unblocks() {
    let (tx1, rx1) = bounded(0);
    let (tx2, rx2) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(500));
            tx2.send(2);
        });

        select! {
            recv(rx1, _) => panic!(),
            recv(rx2, v) => assert_eq!(v, Some(2)),
            default(ms(1000)) => panic!(),
        }
    });

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(500));
            assert_eq!(rx1.recv().unwrap(), 1);
        });

        select! {
            send(tx1, 1) => {},
            send(tx2, 2) => panic!(),
            default(ms(1000)) => panic!(),
        }
    });
}

#[test]
fn both_ready() {
    let (tx1, rx1) = bounded(0);
    let (tx2, rx2) = bounded(0);

    crossbeam::scope(|s| {
        s.spawn(|| {
            thread::sleep(ms(500));
            tx1.send(1);
            assert_eq!(rx2.recv().unwrap(), 2);
        });

        for _ in 0..2 {
            select! {
                recv(rx1, v) => assert_eq!(v, Some(1)),
                send(tx2, 2) => {},
            }
        }
    });
}

#[test]
fn loop_try() {
    for _ in 0..20 {
        let (tx1, rx1) = bounded::<i32>(0);
        let (tx2, rx2) = bounded::<i32>(0);
        let (tx_end, rx_end) = bounded::<()>(0);

        crossbeam::scope(|s| {
            s.spawn(|| {
                loop {
                    select! {
                        send(tx1, 1) => break,
                        default => {}
                    }

                    select! {
                        recv(rx_end, _) => break,
                        default => {}
                    }
                }
            });

            s.spawn(|| {
                loop {
                    if let Some(x) = rx2.try_recv() {
                        assert_eq!(x, 2);
                        break;
                    }

                    select! {
                        recv(rx_end, _) => break,
                        default => {}
                    }
                }
            });

            s.spawn(|| {
                thread::sleep(ms(500));

                select! {
                    recv(rx1, v) => assert_eq!(v, Some(1)),
                    send(tx2, 2) => {},
                    default(ms(500)) => panic!(),
                }

                drop(tx_end);
            });
        });
    }
}

#[test]
fn cloning1() {
    crossbeam::scope(|s| {
        let (tx1, rx1) = unbounded::<i32>();
        let (_tx2, rx2) = unbounded::<i32>();
        let (tx3, rx3) = unbounded::<()>();

        s.spawn(move || {
            rx3.recv().unwrap();
            tx1.clone();
            assert_eq!(rx3.try_recv(), None);
            tx1.send(1);
            rx3.recv().unwrap();
        });

        tx3.send(());

        select! {
            recv(rx1, _) => {},
            recv(rx2, _) => {},
        }

        tx3.send(());
    });
}

#[test]
fn cloning2() {
    let (tx1, rx1) = unbounded::<()>();
    let (tx2, rx2) = unbounded::<()>();
    let (_tx3, _rx3) = unbounded::<()>();

    crossbeam::scope(|s| {
        s.spawn(move || {
            select! {
                recv(rx1, _) => panic!(),
                recv(rx2, _) => {},
            }
        });

        thread::sleep(ms(500));
        drop(tx1.clone());
        tx2.send(());
    })
}

#[test]
fn preflight1() {
    let (tx, rx) = unbounded();
    tx.send(());

    select! {
        recv(rx, _) => {}
    }
}

#[test]
fn preflight2() {
    let (tx, rx) = unbounded();
    drop(tx.clone());
    tx.send(());
    drop(tx);

    select! {
        recv(rx, v) => assert!(v.is_some()),
    }
    assert_eq!(rx.try_recv(), None);
}

#[test]
fn preflight3() {
    let (tx, rx) = unbounded();
    drop(tx.clone());
    tx.send(());
    drop(tx);
    rx.recv().unwrap();

    select! {
        recv(rx, v) => assert!(v.is_none())
    }
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
            for _ in 0..2 {
                select! {
                    recv(rx1, v) => assert_eq!(v, Some(i)),
                    recv(rx2, v) => assert_eq!(v, Some(i)),
                }

                tx3.send(());
            }
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
            for _ in 0..2 {
                select! {
                    send(tx1, i) => {},
                    send(tx2, i) => {},
                }
            }
            tx3.send(());
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
                tx1.send(i);
                assert_eq!(rx2.recv().unwrap(), i);
                rx3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            for _ in 0..2 {
                select! {
                    recv(rx1, v) => assert_eq!(v, Some(i)),
                    send(tx2, i) => {},
                }
            }
            tx3.send(());
        }
    });
}

#[test]
fn stress_timeout_two_threads() {
    const COUNT: usize = 20;

    let (tx, rx) = bounded(2);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(500));
                }

                loop {
                    select! {
                        send(tx, i) => break,
                        default(ms(100)) => {}
                    }
                }
            }
        });

        s.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(500));
                }

                loop {
                    select! {
                        recv(rx, v) => {
                            assert_eq!(v, Some(i));
                            break;
                        }
                        default(ms(100)) => {}
                    }
                }
            }
        });
    });
}

/*
struct WrappedSender<T>(Sender<T>);

impl<T> WrappedSender<T> {
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        select! {
            send(self.0, value) => Ok(()),
            default => Err(TrySendError::Full(value)),
        }
    }

    pub fn send(&self, value: T) {
        select! {
            send(self.0, value) => ()
        }
    }

    pub fn send_timeout(&self, value: T, dur: Duration) -> Result<(), SendTimeoutError<T>> {
        select! {
            send(self.0, value) => Ok(()),
            default(dur) => Err(SendTimeoutError::Timeout(value)),
        }
    }
}

struct WrappedReceiver<T>(Receiver<T>);

impl<T> WrappedReceiver<T> {
    pub fn try_recv(&self) -> Option<T> {
        select! {
            recv(self.0, v) => v,
            default => None,
        }
    }

    pub fn recv(&self) -> Option<T> {
        select! {
            recv(self.0, v) => v
        }
    }

    pub fn recv_timeout(&self, dur: Duration) -> Result<T, RecvTimeoutError> {
        select! {
            recv(self.0, v) => match v {
                Some(v) => Ok(v),
                None => Err(RecvTimeoutError::Closed),
            },
            default(dur) => Err(RecvTimeoutError::Timeout),
        }
    }
}

#[test]
fn recv() {
    let (tx, rx) = bounded(100);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.recv(), Some(7));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(8));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(9));
            assert_eq!(rx.recv(), None);
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            tx.send(7);
            tx.send(8);
            tx.send(9);
        });
    });

    let (tx, rx) = bounded(0);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.recv(), Some(7));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(8));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(9));
            assert_eq!(rx.recv(), None);
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            tx.send(7);
            tx.send(8);
            tx.send(9);
        });
    });
}

#[test]
fn recv_timeout() {
    let (tx, rx) = bounded(100);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.recv_timeout(ms(1000)), Err(RecvTimeoutError::Timeout));
            assert_eq!(rx.recv_timeout(ms(1000)), Ok(7));
            assert_eq!(
                rx.recv_timeout(ms(1000)),
                Err(RecvTimeoutError::Closed)
            );
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            tx.send(7);
        });
    });
}

#[test]
fn try_recv() {
    let (tx, rx) = bounded(100);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.try_recv(), None);
            thread::sleep(ms(1500));
            assert_eq!(rx.try_recv(), Some(7));
            thread::sleep(ms(500));
            assert_eq!(rx.try_recv(), None);
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            tx.send(7);
        });
    });

    let (tx, rx) = bounded(0);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(rx.recv_timeout(ms(1000)), Err(RecvTimeoutError::Timeout));
            assert_eq!(rx.recv_timeout(ms(1000)), Ok(7));
            assert_eq!(
                rx.recv_timeout(ms(1000)),
                Err(RecvTimeoutError::Closed)
            );
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            tx.send(7);
        });
    });
}

#[test]
fn send() {
    let (tx, rx) = bounded(1);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            tx.send(7);
            thread::sleep(ms(1000));
            tx.send(8);
            thread::sleep(ms(1000));
            tx.send(9);
            // TODO: drop(rx) closes the channel
            // thread::sleep(ms(1000));
            // assert_eq!(tx.send(10), Err(SendError(10)));
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(rx.recv(), Some(7));
            assert_eq!(rx.recv(), Some(8));
            assert_eq!(rx.recv(), Some(9));
        });
    });

    let (tx, rx) = bounded(0);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            tx.send(7);
            thread::sleep(ms(1000));
            tx.send(8);
            thread::sleep(ms(1000));
            tx.send(9);
            // TODO: drop(rx) closes the channel
            // assert_eq!(tx.send(10), Err(SendError(10)));
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(rx.recv(), Some(7));
            assert_eq!(rx.recv(), Some(8));
            assert_eq!(rx.recv(), Some(9));
        });
    });
}

#[test]
fn send_timeout() {
    let (tx, rx) = bounded(2);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(tx.send_timeout(1, ms(1000)), Ok(()));
            assert_eq!(tx.send_timeout(2, ms(1000)), Ok(()));
            assert_eq!(
                tx.send_timeout(3, ms(500)),
                Err(SendTimeoutError::Timeout(3))
            );
            thread::sleep(ms(1000));
            assert_eq!(tx.send_timeout(4, ms(1000)), Ok(()));
            // TODO: drop(rx) closes the channel
            // thread::sleep(ms(1000));
            // assert_eq!(tx.send(5), Err(SendError(5)));
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(1));
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(2));
            assert_eq!(rx.recv(), Some(4));
        });
    });

    let (tx, rx) = bounded(0);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(
                tx.send_timeout(7, ms(1000)),
                Err(SendTimeoutError::Timeout(7))
            );
            assert_eq!(tx.send_timeout(8, ms(1000)), Ok(()));
            // TODO: drop(rx) closes the channel
            // assert_eq!(
            //     tx.send_timeout(9, ms(1000)),
            //     Err(SendTimeoutError::Closed(9))
            // );
        });
        s.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(rx.recv(), Some(8));
        });
    });
}

#[test]
fn try_send() {
    let (tx, rx) = bounded(1);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(tx.try_send(1), Ok(()));
            assert_eq!(tx.try_send(2), Err(TrySendError::Full(2)));
            thread::sleep(ms(1500));
            assert_eq!(tx.try_send(3), Ok(()));
            // TODO: drop(rx) closes the channel
            // thread::sleep(ms(500));
            // assert_eq!(tx.try_send(4), Err(TrySendError::Closed(4)));
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(rx.try_recv(), Some(1));
            assert_eq!(rx.try_recv(), None);
            assert_eq!(rx.recv(), Some(3));
        });
    });

    let (tx, rx) = bounded(0);
    let tx = WrappedSender(tx);
    let rx = WrappedReceiver(rx);

    crossbeam::scope(|s| {
        s.spawn(move || {
            assert_eq!(tx.try_send(7), Err(TrySendError::Full(7)));
            thread::sleep(ms(1500));
            assert_eq!(tx.try_send(8), Ok(()));
            // TODO: drop(rx) closes the channel
            // thread::sleep(ms(500));
            // assert_eq!(tx.try_send(9), Err(TrySendError::Closed(9)));
        });
        s.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(rx.recv(), Some(8));
        });
    });
}
*/

// #[test]
// fn recv_after_close() {
//     let (tx, rx) = bounded(100);
//     let tx = WrappedSender(tx);
//     let rx = WrappedReceiver(rx);
//
//     tx.send(1);
//     tx.send(2);
//     tx.send(3);
//
//     drop(tx);
//
//     assert_eq!(rx.recv(), Some(1));
//     assert_eq!(rx.recv(), Some(2));
//     assert_eq!(rx.recv(), Some(3));
//     assert_eq!(rx.recv(), Err(RecvError));
// }

#[test]
fn matching() {
    let (tx, rx) = bounded(0);
    let (tx, rx) = (&tx, &rx);

    crossbeam::scope(|s| {
        for i in 0..44 {
            s.spawn(move || {
                select! {
                    recv(rx, v) => assert_ne!(v.unwrap(), i),
                    send(tx, i) => {},
                }
            });
        }
    });

    assert_eq!(rx.try_recv(), None);
}

#[test]
fn matching_with_leftover() {
    let (tx, rx) = bounded(0);
    let (tx, rx) = (&tx, &rx);

    crossbeam::scope(|s| {
        for i in 0..55 {
            s.spawn(move || {
                select! {
                    recv(rx, v) => assert_ne!(v.unwrap(), i),
                    send(tx, i) => {},
                }
            });
        }
        tx.send(!0);
    });

    assert_eq!(rx.try_recv(), None);
}

#[test]
fn channel_through_channel() {
    const COUNT: usize = 1000;

    type T = Box<Any + Send>;

    for cap in 0..3 {
        let (tx, rx) = bounded::<T>(cap);

        crossbeam::scope(|s| {
            s.spawn(move || {
                let mut tx = tx;

                for _ in 0..COUNT {
                    let (new_tx, new_rx) = bounded(cap);
                    let mut new_rx: T = Box::new(Some(new_rx));

                    select! {
                        send(tx, new_rx) => {}
                    }

                    tx = new_tx;
                }
            });

            s.spawn(move || {
                let mut rx = rx;

                for _ in 0..COUNT {
                    rx = select! {
                        recv(rx, mut r) => {
                            r.unwrap()
                                .downcast_mut::<Option<Receiver<T>>>()
                                .unwrap()
                                .take()
                                .unwrap()
                        }
                    }
                }
            });
        });
    }
}

// #[test]
// fn conditional_send() {
//     let (tx, rx) = bounded(0);
//
//     crossbeam::scope(|s| {
//         s.spawn(move || rx.recv().unwrap());
//
//         select! {
//             send(tx, ()) if 1 + 1 == 3 => panic!(),
//             timed_out(ms(1000)) => {}
//         }
//
//         select! {
//             send(tx, ()) if 1 + 1 == 2 => {},
//             timed_out(ms(1000)) => panic!(),
//         }
//     });
// }
//
// #[test]
// fn conditional_recv() {
//     let (tx, rx) = unbounded();
//     tx.send(());
//
//     select! {
//         recv(rx, _) if 1 + 1 == 3 => panic!(),
//         timed_out(ms(1000)) => {}
//     }
//
//     select! {
//         recv(rx, _) if 1 + 1 == 2 => {},
//         timed_out(ms(1000)) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_closed() {
//     let (_, rx) = bounded::<i32>(0);
//
//     select! {
//         recv(rx, _) => panic!(),
//         closed() if 1 + 1 == 3 => panic!(),
//         would_block() => {}
//         timed_out(ms(100)) => panic!(),
//     }
//
//     select! {
//         recv(rx, _) => panic!(),
//         closed() if 1 + 1 == 2 => {}
//         would_block() => panic!(),
//         timed_out(ms(100)) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_would_block() {
//     let (_tx, rx) = bounded::<i32>(0);
//
//     select! {
//         recv(rx, _) => panic!(),
//         closed() => panic!(),
//         would_block() if 1 + 1 == 3 => panic!(),
//         timed_out(ms(100)) => {}
//     }
//
//     select! {
//         recv(rx, _) => panic!(),
//         closed() => panic!(),
//         would_block() if 1 + 1 == 2 => {}
//         timed_out(ms(100)) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_timed_out() {
//     let (_tx, rx) = bounded::<i32>(0);
//
//     select! {
//         recv(rx, _) => panic!(),
//         timed_out(ms(100)) if 1 + 1 == 3 => panic!(),
//         timed_out(ms(1000)) if 1 + 1 == 2 => {}
//     }
//
//     select! {
//         recv(rx, _) => panic!(),
//         timed_out(ms(100)) if 1 + 1 == 2 => {}
//         timed_out(ms(1000)) if 1 + 1 == 3 => panic!(),
//     }
// }
//
// #[test]
// fn conditional_option_unwrap() {
//     let (tx, rx) = unbounded();
//     tx.send(());
//     let rx = Some(&rx);
//
//     select! {
//         recv(rx.unwrap(), _) if rx.is_some() => {}
//         would_block() => panic!()
//     }
// }
