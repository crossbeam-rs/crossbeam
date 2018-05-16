extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as chan;

use std::any::Any;
use std::thread;
use std::time::Duration;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke1() {
    let (s1, r1) = chan::unbounded::<usize>();
    let (s2, r2) = chan::unbounded::<usize>();

    s1.send(1);

    select! {
        recv(r1, v) => assert_eq!(v, Some(1)),
        recv(r2) => panic!(),
    }

    s2.send(2);

    select! {
        recv(r1) => panic!(),
        recv(r2, v) => assert_eq!(v, Some(2)),
    }
}

#[test]
fn smoke2() {
    let (_s1, r1) = chan::unbounded::<i32>();
    let (_s2, r2) = chan::unbounded::<i32>();
    let (_s3, r3) = chan::unbounded::<i32>();
    let (_s4, r4) = chan::unbounded::<i32>();
    let (s5, r5) = chan::unbounded::<i32>();

    s5.send(5);

    select! {
        recv(r1) => panic!(),
        recv(r2) => panic!(),
        recv(r3) => panic!(),
        recv(r4) => panic!(),
        recv(r5, v) => assert_eq!(v, Some(5)),
    }
}

#[test]
fn closed() {
    let (s1, r1) = chan::unbounded::<i32>();
    let (s2, r2) = chan::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            drop(s1);
            thread::sleep(ms(500));
            s2.send(5);
        });

        select! {
            recv(r1, v) => assert!(v.is_none()),
            recv(r2) => panic!(),
            default(ms(1000)) => panic!(),
        }

        r2.recv().unwrap();
    });

    select! {
        recv(r1, v) => assert!(v.is_none()),
        recv(r2) => panic!(),
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
    let (s1, r1) = chan::unbounded::<i32>();
    let (s2, r2) = chan::unbounded::<i32>();

    select! {
        recv(r1) => panic!(),
        recv(r2) => panic!(),
        default => {}
    }

    drop(s1);

    select! {
        recv(r1, v) => assert!(v.is_none()),
        recv(r2) => panic!(),
        default => panic!(),
    }

    s2.send(2);

    select! {
        recv(r2, v) => assert_eq!(v, Some(2)),
        default => panic!(),
    }

    select! {
        recv(r2) => panic!(),
        default => {},
    }

    select! {
        default => {},
    }
}

#[test]
fn timeout() {
    let (_s1, r1) = chan::unbounded::<i32>();
    let (s2, r2) = chan::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(1500));
            s2.send(2);
        });

        select! {
            recv(r1) => panic!(),
            recv(r2) => panic!(),
            default(ms(1000)) => {},
        }

        select! {
            recv(r1) => panic!(),
            recv(r2, v) => assert_eq!(v, Some(2)),
            default(ms(1000)) => panic!(),
        }
    });

    crossbeam::scope(|scope| {
        let (s, r) = chan::unbounded::<i32>();

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
fn deadline() {
    let (_s1, r1) = chan::unbounded::<i32>();
    let (s2, r2) = chan::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(1500));
            s2.send(2);
        });

        select! {
            recv(r1) => panic!(),
            recv(r2) => panic!(),
            default(Instant::now() + ms(1000)) => {},
        }

        select! {
            recv(r1) => panic!(),
            recv(r2, v) => assert_eq!(v, Some(2)),
            default(Instant::now() + ms(1000)) => panic!(),
        }
    });

    crossbeam::scope(|scope| {
        let (s, r) = chan::unbounded::<i32>();

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
    let (_, r) = chan::unbounded::<i32>();

    select! {
        recv(r, v) => assert!(v.is_none()),
        default => panic!(),
    }

    let (_, r) = chan::unbounded::<i32>();

    select! {
        recv(r, v) => assert!(v.is_none()),
        default(ms(1000)) => panic!(),
    }
}

#[test]
fn unblocks() {
    let (s1, r1) = chan::bounded::<i32>(0);
    let (s2, r2) = chan::bounded::<i32>(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s2.send(2);
        });

        select! {
            recv(r1) => panic!(),
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
    let (s1, r1) = chan::bounded(0);
    let (s2, r2) = chan::bounded(0);

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
        let (s1, r1) = chan::bounded::<i32>(0);
        let (s2, r2) = chan::bounded::<i32>(0);
        let (s_end, r_end) = chan::bounded::<()>(0);

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                loop {
                    select! {
                        send(s1, 1) => break,
                        default => {}
                    }

                    select! {
                        recv(r_end) => break,
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
                        recv(r_end) => break,
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
        let (s1, r1) = chan::unbounded::<i32>();
        let (_s2, r2) = chan::unbounded::<i32>();
        let (s3, r3) = chan::unbounded::<()>();

        scope.spawn(move || {
            r3.recv().unwrap();
            drop(s1.clone());
            assert_eq!(r3.try_recv(), None);
            s1.send(1);
            r3.recv().unwrap();
        });

        s3.send(());

        select! {
            recv(r1) => {},
            recv(r2) => {},
        }

        s3.send(());
    });
}

#[test]
fn cloning2() {
    let (s1, r1) = chan::unbounded::<()>();
    let (s2, r2) = chan::unbounded::<()>();
    let (_s3, _r3) = chan::unbounded::<()>();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            select! {
                recv(r1) => panic!(),
                recv(r2) => {},
            }
        });

        thread::sleep(ms(500));
        drop(s1.clone());
        s2.send(());
    })
}

#[test]
fn preflight1() {
    let (s, r) = chan::unbounded();
    s.send(());

    select! {
        recv(r) => {}
    }
}

#[test]
fn preflight2() {
    let (s, r) = chan::unbounded();
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
    let (s, r) = chan::unbounded();
    drop(s.clone());
    s.send(());
    drop(s);
    r.recv().unwrap();

    select! {
        recv(r, v) => assert!(v.is_none())
    }
}

#[test]
fn duplicate_cases() {
    let (s, r) = chan::unbounded::<i32>();
    let mut hit = [false; 4];

    while hit.iter().any(|hit| !hit) {
        select! {
            recv(r) => hit[0] = true,
            recv(Some(&r)) => hit[1] = true,
            send(s, 0) => hit[2] = true,
            send(Some(&s), 0) => hit[3] = true,
        }
    }
}

#[test]
fn multiple_receivers() {
    let (_, r1) = chan::unbounded::<i32>();
    let (_, r2) = chan::bounded::<i32>(5);
    select! {
        recv([&r1, &r2].iter().map(|x| *x), msg) => assert!(msg.is_none()),
    }
    select! {
        recv([r1, r2].iter(), msg) => assert!(msg.is_none()),
    }

    let (_, r1) = chan::unbounded::<i32>();
    let (_, r2) = chan::bounded::<i32>(5);
    select! {
        recv(&[r1, r2], msg) => assert!(msg.is_none()),
    }
}

#[test]
fn multiple_senders() {
    let (s1, _) = chan::unbounded::<i32>();
    let (s2, _) = chan::bounded::<i32>(5);
    select! {
        send([&s1, &s2].iter().map(|x| *x), 0) => {}
    }
    select! {
        send([s1, s2].iter(), 0) => {}
    }

    let (s1, _) = chan::unbounded::<i32>();
    let (s2, _) = chan::bounded::<i32>(5);
    select! {
        send(&[s1, s2], 0) => {},
    }
}

#[test]
fn recv_handle() {
    let (s1, r1) = chan::unbounded::<i32>();
    let (s2, r2) = chan::unbounded::<i32>();
    let rs = [r1, r2];

    s2.send(0);
    select! {
        recv(rs, _, r) => assert_eq!(r, &s2),
        default => panic!(),
    }

    s1.send(0);
    select! {
        recv(rs, _, r) => assert_eq!(r, &s1),
        default => panic!(),
    }
}

#[test]
fn send_handle() {
    let (s1, r1) = chan::bounded::<i32>(0);
    let (s2, r2) = chan::bounded::<i32>(0);
    let ss = [s1, s2];

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            select! {
                send(ss, 0, s) => assert_eq!(s, &r2),
                default => panic!(),
            }
        });
        r2.recv();
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            select! {
                send(ss, 0, s) => assert_eq!(s, &r1),
                default => panic!(),
            }
        });
        r1.recv();
    });
}

#[test]
fn nesting() {
    let (s, r) = chan::unbounded::<i32>();

    select! {
        send(s, 0) => {
            select! {
                recv(r, v) => {
                    assert_eq!(v, Some(0));
                    select! {
                        send(s, 1) => {
                            select! {
                                recv(r, v) => {
                                    assert_eq!(v, Some(1));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn conditional_send() {
    let (s, _) = chan::unbounded();

    select! {
        send(if 1 + 1 == 3 { Some(&s) } else { None }, ()) => panic!(),
        default(ms(1000)) => {}
    }

    select! {
        send(if 1 + 1 == 2 { Some(&s) } else { None }, ()) => {},
        default(ms(1000)) => panic!(),
    }
}

#[test]
fn conditional_recv() {
    let (s, r) = chan::unbounded();
    s.send(());

    select! {
        recv(if 1 + 1 == 3 { Some(&r) } else { None }) => panic!(),
        default(ms(1000)) => {}
    }

    select! {
        recv(if 1 + 1 == 2 { Some(&r) } else { None }) => {},
        default(ms(1000)) => panic!(),
    }
}

#[test]
fn conditional_default() {
    let (s, r) = chan::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            select! {
                recv(r) => {},
                default(if 1 + 1 == 3 { Some(ms(0)) } else { None }) => panic!(),
            }
        });
        thread::sleep(ms(500));
        s.send(0);
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            select! {
                recv(r) => panic!(),
                default(if 1 + 1 == 2 { Some(ms(0)) } else { None }) => {},
            }
        });
        thread::sleep(ms(500));
        s.send(0);
    });
}

#[test]
fn stress_recv() {
    let (s1, r1) = chan::unbounded();
    let (s2, r2) = chan::bounded(5);
    let (s3, r3) = chan::bounded(100);

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
    let (s1, r1) = chan::bounded(0);
    let (s2, r2) = chan::bounded(0);
    let (s3, r3) = chan::bounded(100);

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
    let (s1, r1) = chan::bounded(0);
    let (s2, r2) = chan::bounded(0);
    let (s3, r3) = chan::bounded(100);

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

    let (s, r) = chan::bounded(2);

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

#[test]
fn matching() {
    let (s, r) = &chan::bounded::<usize>(0);

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
    let (s, r) = &chan::bounded::<usize>(0);

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
        let (s, r) = chan::bounded::<T>(cap);

        crossbeam::scope(|scope| {
            scope.spawn(move || {
                let mut s = s;

                for _ in 0..COUNT {
                    let (new_s, new_r) = chan::bounded(cap);
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
                                .downcast_mut::<Option<chan::Receiver<T>>>()
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

#[test]
fn fairness() {
    const COUNT: usize = 10_000;

    let (s1, r1) = chan::bounded::<()>(COUNT);
    let (s2, r2) = chan::unbounded::<()>();

    for _ in 0..COUNT {
        s1.send(());
        s2.send(());
    }

    let mut hit = [false; 2];
    for _ in 0..COUNT {
        select! {
            recv(r1) => hit[0] = true,
            recv(r2) => hit[1] = true,
        }
    }
    assert!(hit.iter().all(|x| *x));
}
