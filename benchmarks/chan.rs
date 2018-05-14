#[macro_use]
extern crate chan;
extern crate crossbeam;

use chan::{Sender, Receiver};
pub mod testtype;
use testtype::TestType;
const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

type TxRx = (Sender<TestType>, Receiver<TestType>);

fn seq<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    for i in 0..MESSAGES {
        tx.send(TestType::new(i));
    }
    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..MESSAGES {
                tx.send(TestType::new(i));
            }
        });
        s.spawn(|| {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn mpsc<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i));
                }
            });
        }
        s.spawn(|| {
            for _ in 0..MESSAGES {
                rx.recv().unwrap();
            }
        });
    });
}

fn mpmc<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i));
                }
            });
        }
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..MESSAGES / THREADS {
                    rx.recv().unwrap();
                }
            });
        }
    });
}

fn select_rx<F: Fn() -> TxRx>(make: F) {
    let chans = (0..THREADS).map(|_| make()).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(TestType::new(i));
                }
            });
        }

        s.spawn(|| {
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            for _ in 0..MESSAGES {
                chan_select! {
                    rx0.recv() => {},
                    rx1.recv() => {},
                    rx2.recv() => {},
                    rx3.recv() => {},
                }
            }
        });
    });
}

fn select_both<F: Fn() -> TxRx>(make: F) {
    let chans = (0..THREADS).map(|_| make()).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let chans = chans.clone();
            s.spawn(move || {
                let tx0 = &chans[0].0;
                let tx1 = &chans[1].0;
                let tx2 = &chans[2].0;
                let tx3 = &chans[3].0;
                for i in 0..MESSAGES / THREADS {
                    chan_select! {
                        tx0.send(TestType::new(i)) => {},
                        tx1.send(TestType::new(i)) => {},
                        tx2.send(TestType::new(i)) => {},
                        tx3.send(TestType::new(i)) => {},
                    }
                }
            });
        }

        for _ in 0..THREADS {
            let chans = chans.clone();
            s.spawn(move || {
                let rx0 = &chans[0].1;
                let rx1 = &chans[1].1;
                let rx2 = &chans[2].1;
                let rx3 = &chans[3].1;
                for _ in 0..MESSAGES / THREADS {
                    chan_select! {
                        rx0.recv() => {},
                        rx1.recv() => {},
                        rx2.recv() => {},
                        rx3.recv() => {},
                    }
                }
            });
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
                "Rust chan",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("bounded0_mpmc", mpmc(|| chan::sync(0)));
    run!("bounded0_mpsc", mpsc(|| chan::sync(0)));
    // run!("bounded0_select_both", select_both(|| chan::sync(0)));
    run!("bounded0_select_rx", select_rx(|| chan::sync(0)));
    run!("bounded0_spsc", spsc(|| chan::sync(0)));

    run!("bounded1_mpmc", mpmc(|| chan::sync(1)));
    run!("bounded1_mpsc", mpsc(|| chan::sync(1)));
    run!("bounded1_select_both", select_both(|| chan::sync(1)));
    run!("bounded1_select_rx", select_rx(|| chan::sync(1)));
    run!("bounded1_spsc", spsc(|| chan::sync(1)));

    run!("bounded_mpmc", mpmc(|| chan::sync(MESSAGES)));
    run!("bounded_mpsc", mpsc(|| chan::sync(MESSAGES)));
    run!("bounded_select_both", select_both(|| chan::sync(MESSAGES)));
    run!("bounded_select_rx", select_rx(|| chan::sync(MESSAGES)));
    run!("bounded_seq", seq(|| chan::sync(MESSAGES)));
    run!("bounded_spsc", spsc(|| chan::sync(MESSAGES)));

    run!("unbounded_mpmc", mpmc(|| chan::async()));
    run!("unbounded_mpsc", mpsc(|| chan::async()));
    run!("unbounded_select_rx", select_rx(|| chan::async()));
    run!("unbounded_seq", seq(|| chan::async()));
    run!("unbounded_spsc", spsc(|| chan::async()));
    run!("unbounded_select_both", select_both(|| chan::async()));
}
