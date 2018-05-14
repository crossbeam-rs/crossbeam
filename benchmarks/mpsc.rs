#![feature(mpsc_select)]

extern crate crossbeam;

use std::sync::mpsc;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;
pub mod testtype;
use testtype::TestType;

fn seq_async() {
    let (tx, rx) = mpsc::channel::<TestType>();

    for i in 0..MESSAGES {
        tx.send(TestType::new(i)).unwrap();
    }
    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn seq_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel::<TestType>(cap);

    for i in 0..MESSAGES {
        tx.send(TestType::new(i)).unwrap();
    }
    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc_async() {
    let (tx, rx) = mpsc::channel::<TestType>();

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(TestType::new(i)).unwrap();
            }
        });
        s.spawn(move || {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn spsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel::<TestType>(cap);

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(TestType::new(i)).unwrap();
            }
        });
        s.spawn(move || {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn mpsc_async() {
    let (tx, rx) = mpsc::channel::<TestType>();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i)).unwrap();
                }
            });
        }
        s.spawn(move || {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn mpsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel::<TestType>(cap);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i)).unwrap();
                }
            });
        }
        s.spawn(move || {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn select_rx_async() {
    let chans = (0..THREADS).map(|_| mpsc::channel::<TestType>()).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i)).unwrap();
                }
            });
        }

        s.spawn(move || {
            assert!(chans.len() == 4);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            for _ in 0..MESSAGES {
                select! {
                    _ = rx0.recv() => {},
                    _ = rx1.recv() => {},
                    _ = rx2.recv() => {},
                    _ = rx3.recv() => {}
                }
            }
        });
    });
}

fn select_rx_sync(cap: usize) {
    let chans = (0..THREADS).map(|_| mpsc::sync_channel::<TestType>(cap)).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i)).unwrap();
                }
            });
        }

        s.spawn(move || {
            assert!(chans.len() == 4);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            for _ in 0..MESSAGES {
                select! {
                    _ = rx0.recv() => {},
                    _ = rx1.recv() => {},
                    _ = rx2.recv() => {},
                    _ = rx3.recv() => {}
                }
            }
        });
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
