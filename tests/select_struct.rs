//! Tests for the `Select` struct.

extern crate crossbeam;
extern crate crossbeam_channel as channel;

use std::any::Any;
use std::cell::Cell;
use std::thread;
use std::time::Duration;

use channel::Select;

// TODO: verify borrowing in Select<'a, R>
// TODO: verify destructors for closures when wait() is not called
// TODO: verify destructors for closures even when wait() is called

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke1() {
    let (s1, r1) = channel::unbounded::<usize>();
    let (s2, r2) = channel::unbounded::<usize>();

    s1.send(1);

    Select::new()
        .recv(&r1, |v| assert_eq!(v, Some(1)))
        .recv(&r2, |_| panic!())
        .wait();

    s2.send(2);

    Select::new()
        .recv(&r1, |_| panic!())
        .recv(&r2, |v| assert_eq!(v, Some(2)))
        .wait();
}

#[test]
fn smoke2() {
    let (_s1, r1) = channel::unbounded::<i32>();
    let (_s2, r2) = channel::unbounded::<i32>();
    let (_s3, r3) = channel::unbounded::<i32>();
    let (_s4, r4) = channel::unbounded::<i32>();
    let (s5, r5) = channel::unbounded::<i32>();

    s5.send(5);

    Select::new()
        .recv(&r1, |_| panic!())
        .recv(&r2, |_| panic!())
        .recv(&r3, |_| panic!())
        .recv(&r4, |_| panic!())
        .recv(&r5, |v| assert_eq!(v, Some(5)))
        .wait();
}

#[test]
fn closed() {
    let (s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            drop(s1);
            thread::sleep(ms(500));
            s2.send(5);
        });

        let after = channel::after(ms(1000));
        Select::new()
            .recv(&r1, |v| assert!(v.is_none()))
            .recv(&r2, |_| panic!())
            .recv(&after, |_| panic!())
            .wait();

        r2.recv().unwrap();
    });

    let after = channel::after(ms(1000));
    Select::new()
        .recv(&r1, |v| assert!(v.is_none()))
        .recv(&r2, |_| panic!())
        .recv(&after, |_| panic!())
        .wait();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            drop(s2);
        });

        let after = channel::after(ms(1000));
        Select::new()
            .recv(&r2, |v| assert!(v.is_none()))
            .recv(&after, |_| panic!())
            .wait();
    });
}

#[test]
fn default() {
    let (s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();

    Select::new()
        .recv(&r1, |_| panic!())
        .recv(&r2, |_| panic!())
        .default(|| ())
        .wait();

    drop(s1);

    Select::new()
        .recv(&r1, |v| assert!(v.is_none()))
        .recv(&r2, |_| panic!())
        .default(|| panic!())
        .wait();

    s2.send(2);

    Select::new()
        .recv(&r2, |v| assert_eq!(v, Some(2)))
        .default(|| panic!())
        .wait();

    Select::new()
        .recv(&r2, |_| panic!())
        .default(|| ())
        .wait();

    Select::new()
        .default(|| ())
        .wait();
}

#[test]
fn timeout() {
    let (_s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(1500));
            s2.send(2);
        });

        let after = channel::after(ms(1000));
        Select::new()
            .recv(&r1, |_| panic!())
            .recv(&r2, |_| panic!())
            .recv(&after, |_| ())
            .wait();

        let after = channel::after(ms(1000));
        Select::new()
            .recv(&r1, |_| panic!())
            .recv(&r2, |v| assert_eq!(v, Some(2)))
            .recv(&after, |_| panic!())
            .wait();
    });

    crossbeam::scope(|scope| {
        let (s, r) = channel::unbounded::<i32>();

        scope.spawn(move || {
            thread::sleep(ms(500));
            drop(s);
        });

        let after = channel::after(ms(1000));
        Select::new()
            .recv(&after, |_| {
                Select::new()
                    .recv(&r, |v| assert!(v.is_none()))
                    .default(|| panic!())
                    .wait();
            })
            .wait();
    });
}

#[test]
fn default_when_closed() {
    let (_, r) = channel::unbounded::<i32>();

    Select::new()
        .recv(&r, |v| assert!(v.is_none()))
        .default(|| panic!())
        .wait();

    let (_, r) = channel::unbounded::<i32>();

    let after = channel::after(ms(1000));
    Select::new()
        .recv(&r, |v| assert!(v.is_none()))
        .recv(&after, |_| panic!())
        .wait();
}

#[test]
fn unblocks() {
    let (s1, r1) = channel::bounded::<i32>(0);
    let (s2, r2) = channel::bounded::<i32>(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s2.send(2);
        });

        let after = channel::after(ms(1000));
        Select::new()
            .recv(&r1, |_| panic!())
            .recv(&r2, |v| assert_eq!(v, Some(2)))
            .recv(&after, |_| panic!())
            .wait();
    });

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            assert_eq!(r1.recv().unwrap(), 1);
        });

        let after = channel::after(ms(1000));
        Select::new()
            .send(&s1, || 1, || ())
            .send(&s2, || 2, || panic!())
            .recv(&after, |_| panic!())
            .wait();
    });
}

#[test]
fn both_ready() {
    let (s1, r1) = channel::bounded(0);
    let (s2, r2) = channel::bounded(0);

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(ms(500));
            s1.send(1);
            assert_eq!(r2.recv().unwrap(), 2);
        });

        for _ in 0..2 {
            Select::new()
                .recv(&r1, |v| assert_eq!(v, Some(1)))
                .send(&s2, || 2, || ())
                .wait();
        }
    });
}

#[test]
fn loop_try() {
    const RUNS: usize = 20;

    for _ in 0..RUNS {
        let (s1, r1) = channel::bounded::<i32>(0);
        let (s2, r2) = channel::bounded::<i32>(0);
        let (s_end, r_end) = channel::bounded::<()>(0);

        crossbeam::scope(|scope| {
            scope.spawn(|| {
                loop {
                    let mut done = false;

                    Select::new()
                        .send(&s1, || 1, || done = true)
                        .default(|| ())
                        .wait();
                    if done {
                        break;
                    }

                    Select::new()
                        .recv(&r_end, |_| done = true)
                        .default(|| ())
                        .wait();
                    if done {
                        break;
                    }
                }
            });

            scope.spawn(|| {
                loop {
                    if let Some(x) = r2.try_recv() {
                        assert_eq!(x, 2);
                        break;
                    }

                    let mut done = false;
                    Select::new()
                        .recv(&r_end, |_| done = true)
                        .default(|| ())
                        .wait();
                    if done {
                        break;
                    }
                }
            });

            scope.spawn(|| {
                thread::sleep(ms(500));

                let after = channel::after(ms(1000));
                Select::new()
                    .recv(&r1, |v| assert_eq!(v, Some(1)))
                    .send(&s2, || 2, || ())
                    .recv(&after, |_| panic!())
                    .wait();

                drop(s_end);
            });
        });
    }
}

#[test]
fn cloning1() {
    crossbeam::scope(|scope| {
        let (s1, r1) = channel::unbounded::<i32>();
        let (_s2, r2) = channel::unbounded::<i32>();
        let (s3, r3) = channel::unbounded::<()>();

        scope.spawn(move || {
            r3.recv().unwrap();
            drop(s1.clone());
            assert_eq!(r3.try_recv(), None);
            s1.send(1);
            r3.recv().unwrap();
        });

        s3.send(());

        Select::new()
            .recv(&r1, |_| ())
            .recv(&r2, |_| ())
            .wait();

        s3.send(());
    });
}

#[test]
fn cloning2() {
    let (s1, r1) = channel::unbounded::<()>();
    let (s2, r2) = channel::unbounded::<()>();
    let (_s3, _r3) = channel::unbounded::<()>();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            Select::new()
                .recv(&r1, |_| panic!())
                .recv(&r2, |_| ())
                .wait();
        });

        thread::sleep(ms(500));
        drop(s1.clone());
        s2.send(());
    })
}

#[test]
fn preflight1() {
    let (s, r) = channel::unbounded();
    s.send(());

    Select::new()
        .recv(&r, |_| ())
        .wait();
}

#[test]
fn preflight2() {
    let (s, r) = channel::unbounded();
    drop(s.clone());
    s.send(());
    drop(s);

    Select::new()
        .recv(&r, |v| assert!(v.is_some()))
        .wait();
    assert_eq!(r.try_recv(), None);
}

#[test]
fn preflight3() {
    let (s, r) = channel::unbounded();
    drop(s.clone());
    s.send(());
    drop(s);
    r.recv().unwrap();

    Select::new()
        .recv(&r, |v| assert!(v.is_none()))
        .wait();
}

#[test]
fn duplicate_cases() {
    let (s, r) = channel::unbounded::<i32>();
    let hit = vec![Cell::new(false); 4];

    while hit.iter().map(|h| h.get()).any(|hit| !hit) {
        Select::new()
            .recv(&r, |_| hit[0].set(true))
            .recv(&r, |_| hit[1].set(true))
            .send(&s, || 0, || hit[2].set(true))
            .send(&s, || 0, || hit[3].set(true))
            .wait();
    }
}

// #[test]
// fn multiple_receivers() {
//     let (_, r1) = channel::unbounded::<i32>();
//     let (_, r2) = channel::bounded::<i32>(5);
//     select! {
//         recv([&r1, &r2].iter().map(|x| *x), msg) => assert!(msg.is_none()),
//     }
//     select! {
//         recv([r1, r2].iter(), msg) => assert!(msg.is_none()),
//     }
//
//     let (_, r1) = channel::unbounded::<i32>();
//     let (_, r2) = channel::bounded::<i32>(5);
//     select! {
//         recv(&[r1, r2], msg) => assert!(msg.is_none()),
//     }
// }
//
// #[test]
// fn multiple_senders() {
//     let (s1, _) = channel::unbounded::<i32>();
//     let (s2, _) = channel::bounded::<i32>(5);
//     select! {
//         send([&s1, &s2].iter().map(|x| *x), 0) => {}
//     }
//     select! {
//         send([s1, s2].iter(), 0) => {}
//     }
//
//     let (s1, _) = channel::unbounded::<i32>();
//     let (s2, _) = channel::bounded::<i32>(5);
//     select! {
//         send(&[s1, s2], 0) => {},
//     }
// }
//
// #[test]
// fn recv_handle() {
//     let (s1, r1) = channel::unbounded::<i32>();
//     let (s2, r2) = channel::unbounded::<i32>();
//     let rs = [r1, r2];
//
//     s2.send(0);
//     select! {
//         recv(rs, _, r) => assert_eq!(r, &s2),
//         default => panic!(),
//     }
//
//     s1.send(0);
//     select! {
//         recv(rs, _, r) => assert_eq!(r, &s1),
//         default => panic!(),
//     }
// }
//
// #[test]
// fn send_handle() {
//     let (s1, r1) = channel::bounded::<i32>(0);
//     let (s2, r2) = channel::bounded::<i32>(0);
//     let ss = [s1, s2];
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             thread::sleep(ms(500));
//             select! {
//                 send(ss, 0, s) => assert_eq!(s, &r2),
//                 default => panic!(),
//             }
//         });
//         r2.recv();
//     });
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             thread::sleep(ms(500));
//             select! {
//                 send(ss, 0, s) => assert_eq!(s, &r1),
//                 default => panic!(),
//             }
//         });
//         r1.recv();
//     });
// }
//
// #[test]
// fn nesting() {
//     let (s, r) = channel::unbounded::<i32>();
//
//     select! {
//         send(s, 0) => {
//             select! {
//                 recv(r, v) => {
//                     assert_eq!(v, Some(0));
//                     select! {
//                         send(s, 1) => {
//                             select! {
//                                 recv(r, v) => {
//                                     assert_eq!(v, Some(1));
//                                 }
//                             }
//                         }
//                     }
//                 }
//             }
//         }
//     }
// }
//
// #[test]
// fn conditional_send() {
//     let (s, _) = channel::unbounded();
//
//     select! {
//         send(if 1 + 1 == 3 { Some(&s) } else { None }, ()) => panic!(),
//         recv(channel::after(ms(1000))) => {}
//     }
//
//     select! {
//         send(if 1 + 1 == 2 { Some(&s) } else { None }, ()) => {},
//         recv(channel::after(ms(1000))) => panic!(),
//     }
// }
//
// #[test]
// fn conditional_recv() {
//     let (s, r) = channel::unbounded();
//     s.send(());
//
//     select! {
//         recv(if 1 + 1 == 3 { Some(&r) } else { None }) => panic!(),
//         recv(channel::after(ms(1000))) => {}
//     }
//
//     select! {
//         recv(if 1 + 1 == 2 { Some(&r) } else { None }) => {},
//         recv(channel::after(ms(1000))) => panic!(),
//     }
// }
//
// #[test]
// #[should_panic(expected = "send panicked")]
// fn panic_send() {
//     fn get() -> channel::Sender<i32> {
//         panic!("send panicked")
//     }
//
//     select! {
//         send(get(), panic!()) => {}
//     }
// }
//
// #[test]
// #[should_panic(expected = "recv panicked")]
// fn panic_recv() {
//     fn get() -> channel::Receiver<i32> {
//         panic!("recv panicked")
//     }
//
//     select! {
//         recv(get()) => {}
//     }
// }
//
// #[test]
// fn stress_recv() {
//     const COUNT: usize = 10_000;
//
//     let (s1, r1) = channel::unbounded();
//     let (s2, r2) = channel::bounded(5);
//     let (s3, r3) = channel::bounded(100);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             for i in 0..COUNT {
//                 s1.send(i);
//                 r3.recv().unwrap();
//
//                 s2.send(i);
//                 r3.recv().unwrap();
//             }
//         });
//
//         for i in 0..COUNT {
//             for _ in 0..2 {
//                 select! {
//                     recv(r1, v) => assert_eq!(v, Some(i)),
//                     recv(r2, v) => assert_eq!(v, Some(i)),
//                 }
//
//                 s3.send(());
//             }
//         }
//     });
// }
//
// #[test]
// fn stress_send() {
//     const COUNT: usize = 10_000;
//
//     let (s1, r1) = channel::bounded(0);
//     let (s2, r2) = channel::bounded(0);
//     let (s3, r3) = channel::bounded(100);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             for i in 0..COUNT {
//                 assert_eq!(r1.recv().unwrap(), i);
//                 assert_eq!(r2.recv().unwrap(), i);
//                 r3.recv().unwrap();
//             }
//         });
//
//         for i in 0..COUNT {
//             for _ in 0..2 {
//                 select! {
//                     send(s1, i) => {},
//                     send(s2, i) => {},
//                 }
//             }
//             s3.send(());
//         }
//     });
// }
//
// #[test]
// fn stress_mixed() {
//     const COUNT: usize = 10_000;
//
//     let (s1, r1) = channel::bounded(0);
//     let (s2, r2) = channel::bounded(0);
//     let (s3, r3) = channel::bounded(100);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             for i in 0..COUNT {
//                 s1.send(i);
//                 assert_eq!(r2.recv().unwrap(), i);
//                 r3.recv().unwrap();
//             }
//         });
//
//         for i in 0..COUNT {
//             for _ in 0..2 {
//                 select! {
//                     recv(r1, v) => assert_eq!(v, Some(i)),
//                     send(s2, i) => {},
//                 }
//             }
//             s3.send(());
//         }
//     });
// }
//
// #[test]
// fn stress_timeout_two_threads() {
//     const COUNT: usize = 20;
//
//     let (s, r) = channel::bounded(2);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             for i in 0..COUNT {
//                 if i % 2 == 0 {
//                     thread::sleep(ms(500));
//                 }
//
//                 loop {
//                     select! {
//                         send(s, i) => break,
//                         recv(channel::after(ms(100))) => {}
//                     }
//                 }
//             }
//         });
//
//         scope.spawn(|| {
//             for i in 0..COUNT {
//                 if i % 2 == 0 {
//                     thread::sleep(ms(500));
//                 }
//
//                 loop {
//                     select! {
//                         recv(r, v) => {
//                             assert_eq!(v, Some(i));
//                             break;
//                         }
//                         recv(channel::after(ms(100))) => {}
//                     }
//                 }
//             }
//         });
//     });
// }
//
// #[test]
// fn send_recv_same_channel() {
//     let (s, r) = channel::bounded::<i32>(0);
//     select! {
//         send(s, 0) => panic!(),
//         recv(r) => panic!(),
//         recv(channel::after(ms(500))) => {}
//     }
//
//     let (s, r) = channel::unbounded::<i32>();
//     select! {
//         send(s, 0) => {},
//         recv(r) => panic!(),
//         recv(channel::after(ms(500))) => panic!(),
//     }
// }
//
// #[test]
// fn matching() {
//     const THREADS: usize = 44;
//
//     let (s, r) = &channel::bounded::<usize>(0);
//
//     crossbeam::scope(|scope| {
//         for i in 0..THREADS {
//             scope.spawn(move || {
//                 select! {
//                     recv(r, v) => assert_ne!(v.unwrap(), i),
//                     send(s, i) => {},
//                 }
//             });
//         }
//     });
//
//     assert_eq!(r.try_recv(), None);
// }
//
// #[test]
// fn matching_with_leftover() {
//     const THREADS: usize = 55;
//
//     let (s, r) = &channel::bounded::<usize>(0);
//
//     crossbeam::scope(|scope| {
//         for i in 0..THREADS {
//             scope.spawn(move || {
//                 select! {
//                     recv(r, v) => assert_ne!(v.unwrap(), i),
//                     send(s, i) => {},
//                 }
//             });
//         }
//         s.send(!0);
//     });
//
//     assert_eq!(r.try_recv(), None);
// }

#[test]
fn channel_through_channel() {
    const COUNT: usize = 1000;

    type T = Box<Any + Send>;

    for cap in 0..3 {
        let (s, r) = channel::bounded::<T>(cap);

        crossbeam::scope(|scope| {
            scope.spawn(move || {
                let mut s = s;

                for _ in 0..COUNT {
                    let (new_s, new_r) = channel::bounded(cap);
                    let mut new_r: T = Box::new(Some(new_r));

                    Select::new()
                        .send(&s, || new_r, || ())
                        .wait();

                    s = new_s;
                }
            });

            scope.spawn(move || {
                let mut r = r;

                for _ in 0..COUNT {
                    let new = Select::new()
                        .recv(&r, |msg| {
                            msg.unwrap()
                                .downcast_mut::<Option<channel::Receiver<T>>>()
                                .unwrap()
                                .take()
                                .unwrap()
                        })
                        .wait();
                    r = new;
                }
            });
        });
    }
}

// #[test]
// fn linearizable() {
//     const COUNT: usize = 100_000;
//
//     for step in 0..2 {
//         let (start_s, start_r) = channel::bounded::<()>(0);
//         let (end_s, end_r) = channel::bounded::<()>(0);
//
//         let ((s1, r1), (s2, r2)) = if step == 0 {
//             (channel::bounded::<i32>(1), channel::bounded::<i32>(1))
//         } else {
//             (channel::unbounded::<i32>(), channel::unbounded::<i32>())
//         };
//
//         crossbeam::scope(|scope| {
//             scope.spawn(|| {
//                 for _ in 0..COUNT {
//                     start_s.send(());
//
//                     s1.send(1);
//                     select! {
//                         recv(r1) => {}
//                         recv(r2) => {}
//                         default => unreachable!()
//                     }
//
//                     end_s.send(());
//                     r2.try_recv();
//                 }
//             });
//
//             for _ in 0..COUNT {
//                 start_r.recv();
//
//                 s2.send(1);
//                 r1.try_recv();
//
//                 end_r.recv();
//             }
//         });
//     }
// }
//
// #[test]
// fn fairness1() {
//     const COUNT: usize = 10_000;
//
//     let (s1, r1) = channel::bounded::<()>(COUNT);
//     let (s2, r2) = channel::unbounded::<()>();
//
//     for _ in 0..COUNT {
//         s1.send(());
//         s2.send(());
//     }
//
//     let mut hits = [0usize; 4];
//     while hits[0] + hits[1] < 2 * COUNT {
//         select! {
//             recv(r1) => hits[0] += 1,
//             recv(r2) => hits[1] += 1,
//             recv(channel::after(ms(0))) => hits[2] += 1,
//             recv(channel::tick(ms(0))) => hits[3] += 1,
//         }
//     }
//
//     assert!(r1.is_empty());
//     assert!(r2.is_empty());
//
//     let sum: usize = hits.iter().sum();
//     assert!(hits.iter().all(|x| *x >= sum / hits.len() / 2));
// }
//
// #[test]
// fn fairness2() {
//     const COUNT: usize = 10_000;
//
//     let (s1, r1) = channel::unbounded::<()>();
//     let (s2, r2) = channel::bounded::<()>(1);
//     let (s3, r3) = channel::bounded::<()>(0);
//
//     crossbeam::scope(|scope| {
//         scope.spawn(|| {
//             for _ in 0..COUNT {
//                 select! {
//                     send(if s1.is_empty() { Some(&s1) } else { None }, ()) => {}
//                     send(if s2.is_empty() { Some(&s2) } else { None }, ()) => {}
//                     send(s3, ()) => {}
//                 }
//             }
//         });
//
//         let mut hits = [0usize; 3];
//         for _ in 0..COUNT {
//             select! {
//                 recv(r1) => hits[0] += 1,
//                 recv(r2) => hits[1] += 1,
//                 recv(r3) => hits[2] += 1,
//             }
//         }
//         assert!(hits.iter().all(|x| *x >= COUNT / hits.len() / 10));
//     });
// }
