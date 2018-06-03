extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

// TODO: write instructions for porting tests (e.g. runtime.Gosched is thread::yield_now())

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
                let c = channel::bounded(cap);
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
                let c = channel::bounded(cap);
                for i in 0..cap {
                    c.0.send(i);
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
                let c = channel::bounded(cap);
                for i in 0..cap {
                    c.0.send(i);
                }
                drop(c.0);

                for i in 0..cap {
                    let v = c.1.recv();
                    assert_eq!(v, Some(i));
                }

                assert_eq!(c.1.recv(), None);
            }

            {
                // Ensure that close unblocks receive.
                let (s, r) = channel::bounded::<i32>(cap);
                let done = channel::bounded(0);

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
                let c = channel::bounded(cap);
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
                let done = channel::bounded(0);
                crossbeam::scope(|scope| {
                    for _ in 0..P {
                        scope.spawn(|| {
                            for i in 0..L {
                                c.0.send(i);
                            }
                        });
                    }

                    for _ in 0..P {
                        scope.spawn(|| {
                            let mut recv = vec![0; L];
                            for _ in 0..L {
                                let v = c.1.recv().unwrap();
                                recv[v] += 1;
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
                        assert_eq!(v, P);
                    }
                });
            }

            {
                // Test len/cap.
                let c = channel::bounded(cap);

                assert_eq!(c.0.len(), 0);
                assert_eq!(c.0.capacity(), Some(cap));

                for i in 0..cap {
                    c.0.send(i);
                }

                assert_eq!(c.0.len(), cap);
                assert_eq!(c.0.capacity(), Some(cap));
            }
        }
    }

    #[test]
    fn nonblock_recv_race() {
        // TODO
    }

    #[test]
    fn nonblock_select_race() {
        // TODO
    }

    #[test]
    fn nonblock_select_race2() {
        // TODO
    }

    #[test]
    fn self_select() {
        // TODO
    }

    #[test]
    fn select_stress() {
        // TODO
    }

    #[test]
    fn select_fairness() {
        // TODO
    }

    #[test]
    fn chan_send_interface() {
        // TODO
    }

    #[test]
    fn pseudo_random_send() {
        // TODO
    }

    #[test]
    fn multi_consumer() {
        // TODO
    }

    #[test]
    fn shrink_stack_during_blocked_send() {
        // TODO
    }

    #[test]
    fn select_duplicate_channel() {
        // TODO
    }

    #[test]
    fn select_stack_adjust() {
        // TODO
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
