#![feature(mpsc_select)]

extern crate crossbeam;

use shared::{message, shuffle};
use std::sync::mpsc;

mod shared;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq_async() {
    let (tx, rx) = mpsc::channel();

    for i in 0..MESSAGES {
        tx.send(message(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn seq_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel(cap);

    for i in 0..MESSAGES {
        tx.send(message(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc_async() {
    let (tx, rx) = mpsc::channel();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(message(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn spsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel(cap);

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(message(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_async() {
    let (tx, rx) = mpsc::channel();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            scope.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel(cap);

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            scope.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
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
    let mut chans = (0..THREADS).map(|_| mpsc::channel()).collect::<Vec<_>>();

    crossbeam::scope(|scope| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            scope.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            shuffle(&mut chans);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            #[allow(deprecated)]
            {
                select! {
                    m = rx0.recv() => assert!(m.is_ok()),
                    m = rx1.recv() => assert!(m.is_ok()),
                    m = rx2.recv() => assert!(m.is_ok()),
                    m = rx3.recv() => assert!(m.is_ok())
                }
            }
        }
    });
}

fn select_rx_sync(cap: usize) {
    assert_eq!(THREADS, 4);
    let mut chans = (0..THREADS)
        .map(|_| mpsc::sync_channel(cap))
        .collect::<Vec<_>>();

    crossbeam::scope(|scope| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            scope.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            shuffle(&mut chans);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            #[allow(deprecated)]
            {
                select! {
                    m = rx0.recv() => assert!(m.is_ok()),
                    m = rx1.recv() => assert!(m.is_ok()),
                    m = rx2.recv() => assert!(m.is_ok()),
                    m = rx3.recv() => assert!(m.is_ok())
                }
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
        };
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
