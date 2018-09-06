extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

use shared::message;

mod shared;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn new<T>(cap: Option<usize>) -> (channel::Sender<T>, channel::Receiver<T>) {
    match cap {
        None => channel::unbounded(),
        Some(cap) => channel::bounded(cap),
    }
}

fn seq(cap: Option<usize>) {
    let (tx, rx) = new(cap);

    for i in 0..MESSAGES {
        tx.send(message(i));
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc(cap: Option<usize>) {
    let (tx, rx) = new(cap);

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..MESSAGES {
                tx.send(message(i));
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc(cap: Option<usize>) {
    let (tx, rx) = new(cap);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i));
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpmc(cap: Option<usize>) {
    let (tx, rx) = new(cap);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i));
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

fn select_rx(cap: Option<usize>) {
    let chans = (0..THREADS).map(|_| new(cap)).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for (tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i));
                }
            });
        }

        for _ in 0..MESSAGES {
            let mut sel = channel::Select::new();
            for c in &chans {
                sel.recv(&c.1, |msg| assert!(msg.is_some()));
            }
            sel.wait();
            // select! {
            //     recv(chans.iter().map(|c| &c.1), msg, _) => assert!(msg.is_some()),
            // }
        }
    });
}

fn select_both(cap: Option<usize>) {
    let chans = (0..THREADS).map(|_| new(cap)).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for i in 0..MESSAGES / THREADS {
                    // select! {
                    //     send(chans.iter().map(|c| &c.0), message(i), _) => {}
                    // }
                    let mut sel = channel::Select::new();
                    for c in &chans {
                        sel.send(&c.0, || message(i), || ());
                    }
                    sel.wait();
                }
            });
        }

        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..MESSAGES / THREADS {
                    // select! {
                    //     recv(chans.iter().map(|c| &c.1), msg) => assert!(msg.is_some()),
                    // }
                    let mut sel = channel::Select::new();
                    for c in &chans {
                        sel.recv(&c.1, |msg| assert!(msg.is_some()));
                    }
                    sel.wait();
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

    // run!("bounded0_mpmc", mpmc(Some(0)));
    // run!("bounded0_mpsc", mpsc(Some(0)));
    // run!("bounded0_select_both", select_both(Some(0)));
    // run!("bounded0_select_rx", select_rx(Some(0)));
    // run!("bounded0_spsc", spsc(Some(0)));
    //
    // run!("bounded1_mpmc", mpmc(Some(1)));
    // run!("bounded1_mpsc", mpsc(Some(1)));
    // run!("bounded1_select_both", select_both(Some(1)));
    // run!("bounded1_select_rx", select_rx(Some(1)));
    // run!("bounded1_spsc", spsc(Some(1)));
    //
    // run!("bounded_mpmc", mpmc(Some(MESSAGES)));
    // run!("bounded_mpsc", mpsc(Some(MESSAGES)));
    run!("bounded_select_both", select_both(Some(MESSAGES)));
    run!("bounded_select_rx", select_rx(Some(MESSAGES)));
    // run!("bounded_seq", seq(Some(MESSAGES)));
    // run!("bounded_spsc", spsc(Some(MESSAGES)));
    //
    // run!("unbounded_mpmc", mpmc(None));
    // run!("unbounded_mpsc", mpsc(None));
    run!("unbounded_select_both", select_both(None));
    run!("unbounded_select_rx", select_rx(None));
    // run!("unbounded_seq", seq(None));
    // run!("unbounded_spsc", spsc(None));
}
