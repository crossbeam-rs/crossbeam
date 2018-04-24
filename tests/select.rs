extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;

use std::any::Any;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

// TODO: test that `select!` evaluates to an expression
// TODO: two nested `select!`s
// TODO: check `select! { recv(&&&&&r, _) => {} }`

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn bar() {
    let (s, r) = unbounded::<i32>();
    select! {
        // recv(r, msg) => if msg.is_some() {
        // }
        recv(r, msg) => 3.0
        // recv(r, _) => unsafe {
        // }
        // recv(r, _) => loop {
        //     break;
        // }
        // recv(r, _) => match 7 + 3 {
        //     _ => {}
        // }
        default() => 7.
    };
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
                send(s, panic!()) => {}
            }
        }));
        assert_eq!(s.len(), 0);
        drop(s);
    });
}

#[test]
fn same_variable_name() {
    let (_, r) = unbounded::<i32>();
    select! {
        recv(r, r) => assert!(r.is_none()),
    };
    // TODO: do the same with send(s, _, s) and recv(r, _, r)
}

// #[test]
// fn bar() {
//     let (s, r) = bounded(0);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             s.send(8);
//         });
//         scope.spawn(|| {
//             thread::sleep(ms(1000)); /////////////
//             select! {
//                 recv(r, v) => assert_eq!(v, Some(8))
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
//         r0: Receiver<String>,
//         r1: &Receiver<u32>,
//         r2: Receiver<()>,
//         s0: &mut Sender<String>,
//         s1: Sender<String>,
//         s2: Sender<String>,
//         s3: Sender<String>,
//         s4: Sender<String>,
//         s5: Sender<u32>,
//     ) -> Option<String> {
//         select! {
//             recv(r0, val) => Some(val),
//             recv(r1, val) => Some(val.to_string()),
//             recv(r2, ()) => None,
//             send(s0, mut struct_val.0) => Some(var),
//             send(s1, mut var) => Some(struct_val.0),
//             send(s1, immutable_var) => Some(struct_val.0),
//             send(s2, struct_val.0.clone()) => Some(struct_val.0),
//             send(s3, "foo".to_string()) => Some(var),
//             send(s4, var.clone()) => Some(var),
//             send(s5, 42) => None,
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
    let (s1, r1) = unbounded();
    let (s2, r2) = unbounded();

    s1.send(1);

    select! {
        recv(r1, v) => assert_eq!(v, Some(1)),
        recv(r2, _) => panic!(),
    }

    s2.send(2);

    select! {
        recv(r1, _) => panic!(),
        recv(r2, v) => assert_eq!(v, Some(2)),
    }
}

#[test]
fn smoke2() {
    let (_s1, r1) = unbounded::<i32>();
    let (_s2, r2) = unbounded::<i32>();
    let (_s3, r3) = unbounded::<i32>();
    let (_s4, r4) = unbounded::<i32>();
    let (s5, r5) = unbounded::<i32>();

    s5.send(5);

    select! {
        recv(r1, _) => panic!(),
        recv(r2, _) => panic!(),
        recv(r3, _) => panic!(),
        recv(r4, _) => panic!(),
        recv(r5, v) => assert_eq!(v, Some(5)),
    }
}

#[test]
fn closed() {
    let (s1, r1) = unbounded::<i32>();
    let (s2, r2) = unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            drop(s1);
            thread::sleep(ms(500));
            s2.send(5);
        });

        select! {
            recv(r1, v) => assert!(v.is_none()),
            recv(r2, _) => panic!(),
            default(ms(1000)) => panic!(),
        }

        r2.recv().unwrap();
    });

    select! {
        recv(r1, v) => assert!(v.is_none()),
        recv(r2, _) => panic!(),
        default(ms(1000)) => panic!(),
    }

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            drop(s2);
        });

        select! {
            recv(r2, v) => assert!(v.is_none()),
            default(ms(1000)) => panic!(),
        }
    });
}

#[test]
fn default() {
    let (s1, r1) = unbounded::<i32>();
    let (s2, r2) = unbounded::<i32>();

    select! {
        recv(r1, _) => panic!(),
        recv(r2, _) => panic!(),
        default => {}
    }

    drop(s1);

    select! {
        recv(r1, v) => assert!(v.is_none()),
        recv(r2, _) => panic!(),
        default => panic!(),
    }

    s2.send(2);

    select! {
        recv(r2, v) => assert_eq!(v, Some(2)),
        default => panic!(),
    }

    select! {
        recv(r2, _) => panic!(),
        default => {},
    }

    select! {
        default => {},
    }
}

#[test]
fn timeout() {
    let (_s1, r1) = unbounded::<i32>();
    let (s2, r2) = unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(1500));
            s2.send(2);
        });

        select! {
            recv(r1, _) => panic!(),
            recv(r2, _) => panic!(),
            default(ms(1000)) => {},
        }

        select! {
            recv(r1, _) => panic!(),
            recv(r2, v) => assert_eq!(v, Some(2)),
            default(ms(1000)) => panic!(),
        }
    });

    crossbeam::scope(|scope| {
        let (s, r) = unbounded::<i32>();

        scope.spawn(move || {
            thread::sleep(ms(500));
            drop(s);
        });

        select! {
            default(ms(1000)) => {
                select! {
                    recv(r, v) => assert!(v.is_none()),
                    default => panic!(),
                }
            }
        }
    });
}

#[test]
fn default_when_closed() {
    let (_, r) = unbounded::<i32>();

    select! {
        recv(r, v) => assert!(v.is_none()),
        default => panic!(),
    }

    let (_, r) = unbounded::<i32>();

    select! {
        recv(r, v) => assert!(v.is_none()),
        default(ms(1000)) => panic!(),
    }
}

#[test]
fn unblocks() {
    let (s1, r1) = bounded(0);
    let (s2, r2) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s2.send(2);
        });

        select! {
            recv(r1, _) => panic!(),
            recv(r2, v) => assert_eq!(v, Some(2)),
            default(ms(1000)) => panic!(),
        }
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            assert_eq!(r1.recv().unwrap(), 1);
        });

        select! {
            send(s1, 1) => {},
            send(s2, 2) => panic!(),
            default(ms(1000)) => panic!(),
        }
    });
}

#[test]
fn both_ready() {
    let (s1, r1) = bounded(0);
    let (s2, r2) = bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s1.send(1);
            assert_eq!(r2.recv().unwrap(), 2);
        });

        for _ in 0..2 {
            select! {
                recv(r1, v) => assert_eq!(v, Some(1)),
                send(s2, 2) => {},
            }
        }
    });
}

#[test]
fn loop_try() {
    for _ in 0..20 {
        let (s1, r1) = bounded::<i32>(0);
        let (s2, r2) = bounded::<i32>(0);
        let (s_end, r_end) = bounded::<()>(0);

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                loop {
                    select! {
                        send(s1, 1) => break,
                        default => {}
                    }

                    select! {
                        recv(r_end, _) => break,
                        default => {}
                    }
                }
            });

            scope.spawn(|| {
                loop {
                    if let Some(x) = r2.try_recv() {
                        assert_eq!(x, 2);
                        break;
                    }

                    select! {
                        recv(r_end, _) => break,
                        default => {}
                    }
                }
            });

            scope.spawn(|| {
                thread::sleep(ms(500));

                select! {
                    recv(r1, v) => assert_eq!(v, Some(1)),
                    send(s2, 2) => {},
                    default(ms(500)) => panic!(),
                }

                drop(s_end);
            });
        });
    }
}

#[test]
fn cloning1() {
    crossbeam::scope(|scope| {
        let (s1, r1) = unbounded::<i32>();
        let (_s2, r2) = unbounded::<i32>();
        let (s3, r3) = unbounded::<()>();

        scope.spawn(move || {
            r3.recv().unwrap();
            s1.clone();
            assert_eq!(r3.try_recv(), None);
            s1.send(1);
            r3.recv().unwrap();
        });

        s3.send(());

        select! {
            recv(r1, _) => {},
            recv(r2, _) => {},
        }

        s3.send(());
    });
}

#[test]
fn cloning2() {
    let (s1, r1) = unbounded::<()>();
    let (s2, r2) = unbounded::<()>();
    let (_s3, _r3) = unbounded::<()>();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            select! {
                recv(r1, _) => panic!(),
                recv(r2, _) => {},
            }
        });

        thread::sleep(ms(500));
        drop(s1.clone());
        s2.send(());
    })
}

#[test]
fn preflight1() {
    let (s, r) = unbounded();
    s.send(());

    select! {
        recv(r, _) => {}
    }
}

#[test]
fn preflight2() {
    let (s, r) = unbounded();
    drop(s.clone());
    s.send(());
    drop(s);

    select! {
        recv(r, v) => assert!(v.is_some()),
    }
    assert_eq!(r.try_recv(), None);
}

#[test]
fn preflight3() {
    let (s, r) = unbounded();
    drop(s.clone());
    s.send(());
    drop(s);
    r.recv().unwrap();

    select! {
        recv(r, v) => assert!(v.is_none())
    }
}

#[test]
fn stress_recv() {
    let (s1, r1) = unbounded();
    let (s2, r2) = bounded(5);
    let (s3, r3) = bounded(100);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..10_000 {
                s1.send(i);
                r3.recv().unwrap();

                s2.send(i);
                r3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            for _ in 0..2 {
                select! {
                    recv(r1, v) => assert_eq!(v, Some(i)),
                    recv(r2, v) => assert_eq!(v, Some(i)),
                }

                s3.send(());
            }
        }
    });
}

#[test]
fn stress_send() {
    let (s1, r1) = bounded(0);
    let (s2, r2) = bounded(0);
    let (s3, r3) = bounded(100);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..10_000 {
                assert_eq!(r1.recv().unwrap(), i);
                assert_eq!(r2.recv().unwrap(), i);
                r3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            for _ in 0..2 {
                select! {
                    send(s1, i) => {},
                    send(s2, i) => {},
                }
            }
            s3.send(());
        }
    });
}

#[test]
fn stress_mixed() {
    let (s1, r1) = bounded(0);
    let (s2, r2) = bounded(0);
    let (s3, r3) = bounded(100);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..10_000 {
                s1.send(i);
                assert_eq!(r2.recv().unwrap(), i);
                r3.recv().unwrap();
            }
        });

        for i in 0..10_000 {
            for _ in 0..2 {
                select! {
                    recv(r1, v) => assert_eq!(v, Some(i)),
                    send(s2, i) => {},
                }
            }
            s3.send(());
        }
    });
}

#[test]
fn stress_timeout_two_threads() {
    const COUNT: usize = 20;

    let (s, r) = bounded(2);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(500));
                }

                loop {
                    select! {
                        send(s, i) => break,
                        default(ms(100)) => {}
                    }
                }
            }
        });

        scope.spawn(|| {
            for i in 0..COUNT {
                if i % 2 == 0 {
                    thread::sleep(ms(500));
                }

                loop {
                    select! {
                        recv(r, v) => {
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
    let (s, r) = bounded(100);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.recv(), Some(7));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(8));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(9));
            assert_eq!(r.recv(), None);
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            s.send(7);
            s.send(8);
            s.send(9);
        });
    });

    let (s, r) = bounded(0);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.recv(), Some(7));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(8));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(9));
            assert_eq!(r.recv(), None);
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            s.send(7);
            s.send(8);
            s.send(9);
        });
    });
}

#[test]
fn recv_timeout() {
    let (s, r) = bounded(100);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.recv_timeout(ms(1000)), Err(RecvTimeoutError::Timeout));
            assert_eq!(r.recv_timeout(ms(1000)), Ok(7));
            assert_eq!(
                r.recv_timeout(ms(1000)),
                Err(RecvTimeoutError::Closed)
            );
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            s.send(7);
        });
    });
}

#[test]
fn try_recv() {
    let (s, r) = bounded(100);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.try_recv(), None);
            thread::sleep(ms(1500));
            assert_eq!(r.try_recv(), Some(7));
            thread::sleep(ms(500));
            assert_eq!(r.try_recv(), None);
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            s.send(7);
        });
    });

    let (s, r) = bounded(0);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(r.recv_timeout(ms(1000)), Err(RecvTimeoutError::Timeout));
            assert_eq!(r.recv_timeout(ms(1000)), Ok(7));
            assert_eq!(
                r.recv_timeout(ms(1000)),
                Err(RecvTimeoutError::Closed)
            );
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            s.send(7);
        });
    });
}

#[test]
fn send() {
    let (s, r) = bounded(1);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            s.send(7);
            thread::sleep(ms(1000));
            s.send(8);
            thread::sleep(ms(1000));
            s.send(9);
            // TODO: drop(r) closes the channel
            // thread::sleep(ms(1000));
            // assert_eq!(s.send(10), Err(SendError(10)));
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(r.recv(), Some(7));
            assert_eq!(r.recv(), Some(8));
            assert_eq!(r.recv(), Some(9));
        });
    });

    let (s, r) = bounded(0);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            s.send(7);
            thread::sleep(ms(1000));
            s.send(8);
            thread::sleep(ms(1000));
            s.send(9);
            // TODO: drop(r) closes the channel
            // assert_eq!(s.send(10), Err(SendError(10)));
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(r.recv(), Some(7));
            assert_eq!(r.recv(), Some(8));
            assert_eq!(r.recv(), Some(9));
        });
    });
}

#[test]
fn send_timeout() {
    let (s, r) = bounded(2);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(s.send_timeout(1, ms(1000)), Ok(()));
            assert_eq!(s.send_timeout(2, ms(1000)), Ok(()));
            assert_eq!(
                s.send_timeout(3, ms(500)),
                Err(SendTimeoutError::Timeout(3))
            );
            thread::sleep(ms(1000));
            assert_eq!(s.send_timeout(4, ms(1000)), Ok(()));
            // TODO: drop(r) closes the channel
            // thread::sleep(ms(1000));
            // assert_eq!(s.send(5), Err(SendError(5)));
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(1));
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(2));
            assert_eq!(r.recv(), Some(4));
        });
    });

    let (s, r) = bounded(0);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(
                s.send_timeout(7, ms(1000)),
                Err(SendTimeoutError::Timeout(7))
            );
            assert_eq!(s.send_timeout(8, ms(1000)), Ok(()));
            // TODO: drop(r) closes the channel
            // assert_eq!(
            //     s.send_timeout(9, ms(1000)),
            //     Err(SendTimeoutError::Closed(9))
            // );
        });
        scope.spawn(move || {
            thread::sleep(ms(1500));
            assert_eq!(r.recv(), Some(8));
        });
    });
}

#[test]
fn try_send() {
    let (s, r) = bounded(1);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(s.try_send(1), Ok(()));
            assert_eq!(s.try_send(2), Err(TrySendError::Full(2)));
            thread::sleep(ms(1500));
            assert_eq!(s.try_send(3), Ok(()));
            // TODO: drop(r) closes the channel
            // thread::sleep(ms(500));
            // assert_eq!(s.try_send(4), Err(TrySendError::Closed(4)));
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(r.try_recv(), Some(1));
            assert_eq!(r.try_recv(), None);
            assert_eq!(r.recv(), Some(3));
        });
    });

    let (s, r) = bounded(0);
    let s = WrappedSender(s);
    let r = WrappedReceiver(r);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(s.try_send(7), Err(TrySendError::Full(7)));
            thread::sleep(ms(1500));
            assert_eq!(s.try_send(8), Ok(()));
            // TODO: drop(r) closes the channel
            // thread::sleep(ms(500));
            // assert_eq!(s.try_send(9), Err(TrySendError::Closed(9)));
        });
        scope.spawn(move || {
            thread::sleep(ms(1000));
            assert_eq!(r.recv(), Some(8));
        });
    });
}
*/

// #[test]
// fn recv_after_close() {
//     let (s, r) = bounded(100);
//     let s = WrappedSender(s);
//     let r = WrappedReceiver(r);
//
//     s.send(1);
//     s.send(2);
//     s.send(3);
//
//     drop(s);
//
//     assert_eq!(r.recv(), Some(1));
//     assert_eq!(r.recv(), Some(2));
//     assert_eq!(r.recv(), Some(3));
//     assert_eq!(r.recv(), Err(RecvError));
// }

#[test]
fn matching() {
    let (s, r) = bounded(0);
    let (s, r) = (&s, &r);

    crossbeam::scope(|scope| {
        for i in 0..44 {
            scope.spawn(move || {
                select! {
                    recv(r, v) => assert_ne!(v.unwrap(), i),
                    send(s, i) => {},
                }
            });
        }
    });

    assert_eq!(r.try_recv(), None);
}

#[test]
fn matching_with_leftover() {
    let (s, r) = bounded(0);
    let (s, r) = (&s, &r);

    crossbeam::scope(|scope| {
        for i in 0..55 {
            scope.spawn(move || {
                select! {
                    recv(r, v) => assert_ne!(v.unwrap(), i),
                    send(s, i) => {},
                }
            });
        }
        s.send(!0);
    });

    assert_eq!(r.try_recv(), None);
}

#[test]
fn channel_through_channel() {
    const COUNT: usize = 1000;

    type T = Box<Any + Send>;

    for cap in 0..3 {
        let (s, r) = bounded::<T>(cap);

        crossbeam::scope(|scope| {
            scope.spawn(move || {
                let mut s = s;

                for _ in 0..COUNT {
                    let (new_s, new_r) = bounded(cap);
                    let mut new_r: T = Box::new(Some(new_r));

                    select! {
                        send(s, new_r) => {}
                    }

                    s = new_s;
                }
            });

            scope.spawn(move || {
                let mut r = r;

                for _ in 0..COUNT {
                    r = select! {
                        recv(r, mut msg) => {
                            msg.unwrap()
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
//     let (s, r) = bounded(0);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(move || r.recv().unwrap());
//
//         select! {
//             send(s, ()) if 1 + 1 == 3 => panic!(),
//             timed_out(ms(1000)) => {}
//         }
//
//         select! {
//             send(s, ()) if 1 + 1 == 2 => {},
//             timed_out(ms(1000)) => panic!(),
//         }
//     });
// }
//
// #[test]
// fn conditional_recv() {
//     let (s, r) = unbounded();
//     s.send(());
//
//     select! {
//         recv(r, _) if 1 + 1 == 3 => panic!(),
//         timed_out(ms(1000)) => {}
//     }
//
//     select! {
//         recv(r, _) if 1 + 1 == 2 => {},
//         timed_out(ms(1000)) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_closed() {
//     let (_, r) = bounded::<i32>(0);
//
//     select! {
//         recv(r, _) => panic!(),
//         closed() if 1 + 1 == 3 => panic!(),
//         would_block() => {}
//         timed_out(ms(100)) => panic!(),
//     }
//
//     select! {
//         recv(r, _) => panic!(),
//         closed() if 1 + 1 == 2 => {}
//         would_block() => panic!(),
//         timed_out(ms(100)) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_would_block() {
//     let (_s, r) = bounded::<i32>(0);
//
//     select! {
//         recv(r, _) => panic!(),
//         closed() => panic!(),
//         would_block() if 1 + 1 == 3 => panic!(),
//         timed_out(ms(100)) => {}
//     }
//
//     select! {
//         recv(r, _) => panic!(),
//         closed() => panic!(),
//         would_block() if 1 + 1 == 2 => {}
//         timed_out(ms(100)) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_timed_out() {
//     let (_s, r) = bounded::<i32>(0);
//
//     select! {
//         recv(r, _) => panic!(),
//         timed_out(ms(100)) if 1 + 1 == 3 => panic!(),
//         timed_out(ms(1000)) if 1 + 1 == 2 => {}
//     }
//
//     select! {
//         recv(r, _) => panic!(),
//         timed_out(ms(100)) if 1 + 1 == 2 => {}
//         timed_out(ms(1000)) if 1 + 1 == 3 => panic!(),
//     }
// }
//
// #[test]
// fn conditional_option_unwrap() {
//     let (s, r) = unbounded();
//     s.send(());
//     let r = Some(&r);
//
//     select! {
//         recv(r.unwrap(), _) if r.is_some() => {}
//         would_block() => panic!()
//     }
// }
