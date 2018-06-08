#![feature(mpsc_select)]

extern crate crossbeam;

use std::sync::mpsc;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq_async() {
    let (tx, rx) = mpsc::channel::<i32>();

    for i in 0..MESSAGES {
        tx.send(i as i32).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

pub fn shuffle<T>(v: &mut [T]) {
    use std::cell::Cell;
    use std::num::Wrapping;

    let len = v.len();
    if len <= 1 {
        return;
    }

    thread_local! {
        static RNG: Cell<Wrapping<u32>> = Cell::new(Wrapping(1));
    }

    RNG.with(|rng| {
        for i in 1..len {
            // This is the 32-bit variant of Xorshift.
            // https://en.wikipedia.org/wiki/Xorshift
            let mut x = rng.get();
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            rng.set(x);

            let x = x.0;
            let n = i + 1;

            // This is a fast alternative to `let j = x % n`.
            // https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
            let j = ((x as u64 * n as u64) >> 32) as u32 as usize;

            v.swap(i, j);
        }
    });
}

fn seq_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel::<i32>(cap);

    for i in 0..MESSAGES {
        tx.send(i as i32).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc_async() {
    let (tx, rx) = mpsc::channel::<i32>();

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(i as i32).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn spsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel::<i32>(cap);

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(i as i32).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_async() {
    let (tx, rx) = mpsc::channel::<i32>();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel::<i32>(cap);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn select_rx_async() {
    assert_eq!(THREADS, 4);
    let mut chans = (0..THREADS).map(|_| mpsc::channel::<i32>()).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            shuffle(&mut chans);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            select! {
                m = rx0.recv() => assert!(m.is_ok()),
                m = rx1.recv() => assert!(m.is_ok()),
                m = rx2.recv() => assert!(m.is_ok()),
                m = rx3.recv() => assert!(m.is_ok())
            };
        }
    });
}

fn select_rx_sync(cap: usize) {
    assert_eq!(THREADS, 4);
    let mut chans = (0..THREADS).map(|_| mpsc::sync_channel::<i32>(cap)).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            shuffle(&mut chans);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            select! {
                m = rx0.recv() => assert!(m.is_ok()),
                m = rx1.recv() => assert!(m.is_ok()),
                m = rx2.recv() => assert!(m.is_ok()),
                m = rx3.recv() => assert!(m.is_ok())
            }
        }
    });
}

fn main() {
    macro_rules! run {
        ($name:expr, $f:expr) => {
            let now = ::std::time::Instant::now();
            $f;
            let elapsed = now.elapsed();
            println!(
                "{:25} {:15} {:7.3} sec",
                $name,
                "Rust mpsc",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("bounded0_mpsc", mpsc_sync(0));
    run!("bounded0_select_rx", select_rx_sync(0));
    run!("bounded0_spsc", spsc_sync(0));

    run!("bounded1_mpsc", mpsc_sync(1));
    run!("bounded1_select_rx", select_rx_sync(1));
    run!("bounded1_spsc", spsc_sync(1));

    run!("bounded_mpsc", mpsc_sync(MESSAGES));
    run!("bounded_select_rx", select_rx_sync(MESSAGES));
    run!("bounded_seq", seq_sync(MESSAGES));
    run!("bounded_spsc", spsc_sync(MESSAGES));

    run!("unbounded_mpsc", mpsc_async());
    run!("unbounded_select_rx", select_rx_async());
    run!("unbounded_seq", seq_async());
    run!("unbounded_spsc", spsc_async());
}
