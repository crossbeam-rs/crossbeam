//! Tests borrowed from Go and ported to Rust.

extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;
extern crate parking_lot;

mod wrappers;

macro_rules! tests {
    ($channel:path) => {
        use std::any::Any;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::thread;
        use std::time::Duration;

        use $channel as channel;
        use crossbeam;
        use parking_lot::Mutex;

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        // https://github.com/golang/go/blob/master/test/chan/doubleselect.go
        mod doubleselect {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/fifo.go
        mod fifo {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/nonblock.go
        mod nonblock {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select.go
        mod select {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select2.go
        mod select2 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select3.go
        mod select3 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select4.go
        mod select4 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select5.go
        mod select5 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select6.go
        mod select6 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/select7.go
        mod select7 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/sieve1.go
        mod sieve1 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/sieve2.go
        mod sieve2 {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/chan/zerosize.go
        mod zerosize {
            // TODO
        }

        // https://github.com/golang/go/blob/master/src/runtime/chan_test.go
        mod chan_test {
            use super::*;

            #[test]
            fn chan() {
                const N: usize = 200;

                for cap in 0..N {
                    {
                        let c = channel::bounded::<i32>(cap);
                        let recv1 = AtomicBool::new(false);
                        let recv2 = AtomicBool::new(false);

                        crossbeam::scope(|scope| {
                            scope.spawn(|| {
                                c.1.recv();
                                recv1.store(true, Ordering::SeqCst);
                            });
                            scope.spawn(|| {
                                c.1.recv();
                                recv2.store(true, Ordering::SeqCst);
                            });

                            thread::sleep(ms(1));

                            if recv1.load(Ordering::SeqCst) || recv2.load(Ordering::SeqCst) {
                                panic!();
                            }

                            // Ensure that non-blocking receive does not block.
                            select! {
                                recv(c.1) => panic!(),
                                default => {}
                            }
                            select! {
                                recv(c.1) => panic!(),
                                default => {}
                            }

                            c.0.send(0);
                            c.0.send(0);
                        });
                    }

                    {
                        // Ensure that send to full chan blocks.
                        let c = channel::bounded::<i32>(cap);
                        for i in 0..cap {
                            c.0.send(i as i32);
                        }
                        let sent = AtomicUsize::new(0);

                        crossbeam::scope(|scope| {
                            scope.spawn(|| {
                                c.0.send(0);
                                sent.store(1, Ordering::SeqCst);
                            });

                            thread::sleep(ms(1));

                            if sent.load(Ordering::SeqCst) != 0 {
                                panic!();
                            }

                            // Ensure that non-blocking send does not block.
                            select! {
                                send(c.0, 0) => panic!(),
                                default => {}
                            }
                            c.1.recv();
                        });
                    }

                    {
                        // Ensure that we receive 0 from closed chan.
                        let c = channel::bounded::<i32>(cap);
                        for i in 0..cap {
                            c.0.send(i as i32);
                        }
                        drop(c.0);

                        for i in 0..cap {
                            let v = c.1.recv();
                            assert_eq!(v, Some(i as i32));
                        }

                        assert_eq!(c.1.recv(), None);
                    }

                    {
                        // Ensure that close unblocks receive.
                        let (s, r) = channel::bounded::<i32>(cap);
                        let done = channel::bounded::<bool>(0);

                        crossbeam::scope(|scope| {
                            scope.spawn(|| done.0.send(r.recv() == None));
                            thread::sleep(ms(1));
                            drop(s);

                            assert_eq!(done.1.recv(), Some(true));
                        });
                    }

                    {
                        // Send 100 integers,
                        // ensure that we receive them non-corrupted in FIFO order.
                        let c = channel::bounded::<i32>(cap);
                        crossbeam::scope(|scope| {
                            scope.spawn(|| {
                                for i in 0..100 {
                                    c.0.send(i);
                                }
                            });

                            for i in 0..100 {
                                assert_eq!(c.1.recv(), Some(i));
                            }
                        });

                        // Same, but using recv2.
                        crossbeam::scope(|scope| {
                            scope.spawn(|| {
                                for i in 0..100 {
                                    c.0.send(i);
                                }
                            });

                            for i in 0..100 {
                                assert_eq!(c.1.recv(), Some(i));
                            }
                        });

                        // Send 1000 integers in 4 goroutines,
                        // ensure that we receive what we send.
                        const P: usize = 4;
                        const L: usize = 1000;
                        let done = channel::bounded::<Vec<i32>>(0);
                        crossbeam::scope(|scope| {
                            for _ in 0..P {
                                scope.spawn(|| {
                                    for i in 0..L {
                                        c.0.send(i as i32);
                                    }
                                });
                            }

                            for _ in 0..P {
                                scope.spawn(|| {
                                    let mut recv = vec![0; L];
                                    for _ in 0..L {
                                        let v = c.1.recv().unwrap();
                                        recv[v as usize] += 1;
                                    }
                                    done.0.send(recv);
                                });
                            }

                            let mut recv = vec![0; L];
                            for _ in 0..P {
                                for (i, v) in done.1.recv().unwrap().into_iter().enumerate() {
                                    recv[i] += v;
                                }
                            }

                            assert_eq!(recv.len(), L);
                            for v in recv {
                                assert_eq!(v, P as i32);
                            }
                        });
                    }

                    {
                        // Test len/cap.
                        let c = channel::bounded::<i32>(cap);

                        assert_eq!(c.0.len(), 0);
                        assert_eq!(c.0.capacity(), Some(cap));

                        for i in 0..cap {
                            c.0.send(i as i32);
                        }

                        assert_eq!(c.0.len(), cap);
                        assert_eq!(c.0.capacity(), Some(cap));
                    }
                }
            }

            #[test]
            fn nonblock_recv_race() {
                const N: usize = 10000;

                for _ in 0..N {
                    let (s, r) = channel::bounded(1);
                    s.send(1);

                    crossbeam::scope(|scope| {
                        scope.spawn(|| {
                            select! {
                                recv(r) => {}
                                default => panic!("chan is not ready"),
                            }
                        });

                        drop(s);
                        r.recv();
                    });
                }
            }

            #[test]
            fn nonblock_select_race() {
                const N: usize = 10000;

                let (done_s, done_r) = channel::bounded::<bool>(1);
                for _ in 0..N {
                    let (s1, r1) = channel::bounded::<i32>(1);
                    let (s2, r2) = channel::bounded::<i32>(1);
                    s1.send(1);

                    crossbeam::scope(|scope| {
                        scope.spawn(|| {
                            select! {
                                recv(r1) => {}
                                recv(r2) => {}
                                default => {
                                    done_s.send(false);
                                    return;
                                }
                            }
                            done_s.send(true);
                        });

                        s2.send(1);
                        select! {
                            recv(r1) => {}
                            default => {}
                        }

                        if !done_r.recv().unwrap() {
                            panic!("no chan is ready");
                        }
                    });
                }
            }

            #[test]
            fn nonblock_select_race2() {
                const N: usize = 1000;

                let (done_s, done_r) = channel::bounded::<bool>(1);
                for _ in 0..N {
                    let (s1, r1) = channel::bounded::<i32>(1);
                    let (s2, r2) = channel::bounded::<i32>(0);
                    s1.send(1);

                    crossbeam::scope(|scope| {
                        scope.spawn(|| {
                            select! {
                                recv(r1) => {}
                                recv(r2) => {}
                                default => {
                                    done_s.send(false);
                                    return;
                                }
                            }
                            done_s.send(true);
                        });

                        drop(s2);
                        select! {
                            recv(r1) => {}
                            default => {}
                        }

                        if !done_r.recv().unwrap() {
                            panic!("no chan is ready");
                        }
                    });
                }
            }

            #[test]
            fn self_select() {
                // Ensure that send/recv on the same chan in select
                // does not crash nor deadlock.
                for &cap in &[0, 10] {
                    let (s, r) = channel::bounded::<i32>(cap);

                    crossbeam::scope(|scope| {
                        for p in 0..2 {
                            let p = p;
                            let (s, r) = (&s, &r);
                            scope.spawn(move || {
                                for i in 0..1000 {
                                    if p == 0 || i % 2 == 0 {
                                        select! {
                                            send(s, p) => {}
                                            recv(r, v) => {
                                                if cap == 0 && v.unwrap() == p {
                                                    panic!("self receive");
                                                }
                                            }
                                        }
                                    } else {
                                        select! {
                                            recv(r, v) => {
                                                if cap == 0 && v.unwrap() == p {
                                                    panic!("self receive");
                                                }
                                            }
                                            send(s, p) => {}
                                        }
                                    }
                                }
                            });
                        }
                    });
                }
            }

            #[test]
            fn select_stress() {
                let c = vec![
                    channel::bounded(0),
                    channel::bounded(0),
                    channel::bounded(2),
                    channel::bounded(3),
                ];

                const N: usize = 10000;

                // There are 4 goroutines that send N values on each of the chans,
                // + 4 goroutines that receive N values on each of the chans,
                // + 1 goroutine that sends N values on each of the chans in a single select,
                // + 1 goroutine that receives N values on each of the chans in a single select.
                // All these sends, receives and selects interact chaotically at runtime,
                // but we are careful that this whole construct does not deadlock.
                crossbeam::scope(|scope| {
                    for k in 0..4 {
                        {
                            let c = c.clone();
                            let k = k;
                            scope.spawn(move || {
                                for _ in 0..N {
                                    c[k].0.send(0);
                                }
                            });
                        }
                        {
                            let c = c.clone();
                            let k = k;
                            scope.spawn(move || {
                                for _ in 0..N {
                                    c[k].1.recv();
                                }
                            });
                        }
                    }

                    {
                        let mut s = c.iter()
                            .map(|(s, _)| Some(s.clone()))
                            .collect::<Vec<_>>();

                        scope.spawn(move || {
                            let mut n = [0i32; 4];
                            for _ in 0..4 * N {
                                let i;
                                select! {
                                    send(s[3].iter().map(|x| &**x), 0) => i = 3,
                                    send(s[2].iter().map(|x| &**x), 0) => i = 2,
                                    send(s[0].iter().map(|x| &**x), 0) => i = 0,
                                    send(s[1].iter().map(|x| &**x), 0) => i = 1,
                                }
                                n[i] += 1;
                                assert!(n[i] <= N as i32);
                                if n[i] == N as i32 {
                                    s[i] = None;
                                }
                            }
                        });
                    }

                    {
                        let mut r = c.iter()
                            .map(|(_, r)| Some(r.clone()))
                            .collect::<Vec<_>>();

                        scope.spawn(move || {
                            let mut n = [0i32; 4];
                            for _ in 0..4 * N {
                                let i;
                                select! {
                                    recv(r[0].iter().map(|x| &**x)) => i = 0,
                                    recv(r[1].iter().map(|x| &**x)) => i = 1,
                                    recv(r[2].iter().map(|x| &**x)) => i = 2,
                                    recv(r[3].iter().map(|x| &**x)) => i = 3,
                                }
                                n[i] += 1;
                                assert!(n[i] <= N as i32);
                                if n[i] == N as i32 {
                                    r[i] = None;
                                }
                            }
                        });
                    }
                });
            }

            #[test]
            fn select_fairness() {
                const TRIALS: usize = 10000;

                let (s1, r1) = channel::bounded::<u8>(TRIALS + 1);
                let (s2, r2) = channel::bounded::<u8>(TRIALS + 1);

                for _ in 0..TRIALS + 1 {
                    s1.send(1);
                    s2.send(2);
                }

                let (_s3, r3) = channel::bounded::<u8>(TRIALS + 1);
                let (_s4, r4) = channel::bounded::<u8>(TRIALS + 1);
                let (out_s, out_r) = channel::bounded::<u8>(TRIALS + 1);
                let (done_s, done_r) = channel::bounded::<u8>(TRIALS + 1);

                crossbeam::scope(|scope| {
                    scope.spawn(|| {
                        loop {
                            let b = select! {
                                recv(r3, m) => m,
                                recv(r4, m) => m,
                                recv(r1, m) => m,
                                recv(r2, m) => m,
                            }.unwrap();

                            select! {
                                send(out_s, b) => {}
                                recv(done_r) => return
                            }
                        }
                    });

                    let (mut cnt1, mut cnt2) = (0, 0);
                    for _ in 0..TRIALS {
                        match out_r.recv() {
                            Some(1) => cnt1 += 1,
                            Some(2) => cnt2 += 1,
                            b => panic!("unexpected value {:?} on channel", b),
                        }
                    }

                    // If the select in the goroutine is fair,
                    // cnt1 and cnt2 should be about the same value.
                    // With 10,000 trials, the expected margin of error at
                    // a confidence level of five nines is 4.4172 / (2 * Sqrt(10000)).

                    let r = cnt1 as f64 / TRIALS as f64;
                    let e = (r - 0.5).abs();

                    if e > 4.4172 / (2.0 * (TRIALS as f64).sqrt()) {
                        panic!(
                            "unfair select: in {} trials, results were {}, {}",
                            TRIALS,
                            cnt1,
                            cnt2,
                        );
                    }

                    drop(done_s);
                });
            }

            #[test]
            fn chan_send_interface() {
                struct Mt;

                let (s, _r) = channel::bounded::<Box<Any>>(1);
                s.send(Box::new(Mt));

                select! {
                    send(s, Box::new(Mt)) => {}
                    default => {}
                }

                select! {
                    send(s, Box::new(Mt)) => {}
                    send(s, Box::new(Mt)) => {}
                    default => {}
                }
            }

            #[test]
            fn pseudo_random_send() {
                const N: usize = 100;

                for cap in 0..N {
                    let (s, r) = channel::bounded::<i32>(cap);
                    let l = Mutex::new(vec![0i32; N]);

                    crossbeam::scope(|scope| {
                        scope.spawn(|| {
                            let mut l = l.lock();
                            for i in 0..N {
                                thread::yield_now();
                                l[i] = r.recv().unwrap();
                            }
                        });

                        for _ in 0..N {
                            select! {
                                send(s, 1) => {}
                                send(s, 0) => {}
                            }
                        }

                        let l = l.lock();
                        let mut n0 = 0;
                        let mut n1 = 0;
                        for &i in l.iter() {
                            n0 += (i + 1) % 2;
                            n1 += i;
                        }

                        if n0 <= N as i32 / 10 || n1 <= N as i32 / 10 {
                            panic!(
                                "Want pseudorandom, got {} zeros and {} ones (chan cap {})",
                                n0,
                                n1,
                                cap,
                            );
                        }
                    });
                }
            }

            #[test]
            fn multi_consumer() {
                const NWORK: usize = 23;
                const NITER: usize = 271828;

                let pn = [2, 3, 7, 11, 13, 17, 19, 23, 27, 31];

                let (q_s, q_r) = channel::bounded::<i32>(NWORK * 3);
                let (r_s, r_r) = channel::bounded::<i32>(NWORK * 3);

                let expect = AtomicUsize::new(0);

                crossbeam::scope(|scope| {
                    // workers
                    for i in 0..NWORK {
                        let w = i;
                        let q_r = &q_r;
                        let pn = &pn;
                        let r_s = r_s.clone();
                        scope.spawn(move || {
                            for v in &q_r.0 {
                                // mess with the fifo-ish nature of range
                                if pn[w % pn.len()] == v {
                                    thread::yield_now();
                                }
                                r_s.send(v);
                            }
                        });
                    }

                    // feeder & closer
                    scope.spawn(|| {
                        for i in 0..NITER {
                            let v = pn[i % pn.len()];
                            expect.fetch_add(v as usize, Ordering::SeqCst);
                            q_s.send(v);
                        }

                        drop(q_s);
                        drop(r_s);
                    });

                    // consume & check
                    let mut n = 0;
                    let mut s = 0;
                    for v in &r_r.0 {
                        n += 1;
                        s += v;
                    }
                    if n != NITER || s != expect.load(Ordering::SeqCst) as i32 {
                        panic!(
                            "Expected sum {} (got {}) from {} iter (saw {})",
                            expect.load(Ordering::SeqCst),
                            s,
                            NITER,
                            n,
                        );
                    }
                });
            }

            #[test]
            fn select_duplicate_channel() {
                // This test makes sure we can queue a G on
                // the same channel multiple times.
                let (c_s, c_r) = channel::bounded::<i32>(0);
                let (d_s, d_r) = channel::bounded::<i32>(0);
                let (e_s, e_r) = channel::bounded::<i32>(0);

                crossbeam::scope(|scope| {
                    scope.spawn(|| {
                        select! {
                            recv(c_r) => {}
                            recv(c_r) => {}
                            recv(d_r) => {}
                        }
                        e_s.send(9);
                    });
                    thread::sleep(ms(1));

                    scope.spawn(|| {
                        c_r.recv();
                    });
                    thread::sleep(ms(1));

                    d_s.send(7);
                    e_r.recv();
                    c_s.send(8);
                });
            }
        }

        // https://github.com/golang/go/blob/master/test/closedchan.go
        mod closedchan {
            // TODO
        }

        // https://github.com/golang/go/blob/master/src/runtime/chanbarrier_test.go
        mod chanbarrier_test {
            // TODO
        }

        // https://github.com/golang/go/blob/master/src/runtime/race/testdata/chan_test.go
        mod race_chan_test {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/ken/chan.go
        mod chan {
            // TODO
        }

        // https://github.com/golang/go/blob/master/test/ken/chan1.go
        mod chan1 {
            // TODO
        }
    }
}

mod normal {
    tests!(wrappers::normal);
}

mod cloned {
    tests!(wrappers::cloned);
}

mod select {
    tests!(wrappers::select);
}

mod select_spin {
    tests!(wrappers::select_spin);
}

mod select_multi {
    tests!(wrappers::select_multi);
}
