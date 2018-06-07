extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

type TxRx = (channel::Sender<i32>, channel::Receiver<i32>);

fn seq<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    for i in 0..MESSAGES {
        tx.send(i as i32);
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
                tx.send(i as i32);
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32);
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpmc<F: Fn() -> TxRx>(make: F) {
    let (tx, rx) = make();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32);
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
        for (tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(i as i32);
                }
            });
        }

        for _ in 0..MESSAGES {
            select! {
                recv(chans.iter().map(|c| &c.1), msg, _) => assert!(msg.is_some()),
            }
        }
    });
}

fn select_both<F: Fn() -> TxRx>(make: F) {
    let chans = (0..THREADS).map(|_| make()).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    select! {
                        send(chans.iter().map(|c| &c.0), i as i32, _) => {}
                    }
                }
            });
        }

        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..MESSAGES / THREADS {
                    select! {
                        recv(chans.iter().map(|c| &c.1), msg) => assert!(msg.is_some()),
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
                "Rust crossbeam-channel",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("bounded0_mpmc", mpmc(|| channel::bounded(0)));
    run!("bounded0_mpsc", mpsc(|| channel::bounded(0)));
    run!("bounded0_select_both", select_both(|| channel::bounded(0)));
    run!("bounded0_select_rx", select_rx(|| channel::bounded(0)));
    run!("bounded0_spsc", spsc(|| channel::bounded(0)));

    run!("bounded1_mpmc", mpmc(|| channel::bounded(1)));
    run!("bounded1_mpsc", mpsc(|| channel::bounded(1)));
    run!("bounded1_select_both", select_both(|| channel::bounded(1)));
    run!("bounded1_select_rx", select_rx(|| channel::bounded(1)));
    run!("bounded1_spsc", spsc(|| channel::bounded(1)));

    run!("bounded_mpmc", mpmc(|| channel::bounded(MESSAGES)));
    run!("bounded_mpsc", mpsc(|| channel::bounded(MESSAGES)));
    run!("bounded_select_both", select_both(|| channel::bounded(MESSAGES)));
    run!("bounded_select_rx", select_rx(|| channel::bounded(MESSAGES)));
    run!("bounded_seq", seq(|| channel::bounded(MESSAGES)));
    run!("bounded_spsc", spsc(|| channel::bounded(MESSAGES)));

    run!("unbounded_mpmc", mpmc(|| channel::unbounded()));
    run!("unbounded_mpsc", mpsc(|| channel::unbounded()));
    run!("unbounded_select_both", select_both(|| channel::unbounded()));
    run!("unbounded_select_rx", select_rx(|| channel::unbounded()));
    run!("unbounded_seq", seq(|| channel::unbounded()));
    run!("unbounded_spsc", spsc(|| channel::unbounded()));
}
