extern crate crossbeam;
extern crate futures;

use futures::channel::mpsc;
use futures::executor::ThreadPool;
use futures::prelude::*;
use futures::{future, stream, SinkExt, StreamExt};

mod message;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq_unbounded() {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|_| {
            let (tx, rx) = mpsc::unbounded();
            for i in 0..MESSAGES {
                tx.unbounded_send(message::new(i)).unwrap();
            }
            drop(tx);

            rx.for_each(|_| future::ok(()))
        }))
        .unwrap();
}

fn seq_bounded(cap: usize) {
    let (mut tx, rx) = mpsc::channel(cap);
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|_| {
            for i in 0..MESSAGES {
                tx.try_send(message::new(i)).unwrap();
            }
            drop(tx);

            rx.for_each(|_| future::ok(()))
        }))
        .unwrap();
}

fn spsc_unbounded() {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|cx| {
            let (tx, rx) = mpsc::unbounded();

            cx.spawn(future::lazy(move |_| {
                tx.send_all(stream::iter_ok((0..MESSAGES).map(|i| message::new(i))))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
            }));

            rx.for_each(|_| future::ok(()))
        }))
        .unwrap();
}

fn spsc_bounded(cap: usize) {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|cx| {
            let (tx, rx) = mpsc::channel(cap);

            cx.spawn(future::lazy(move |_| {
                tx.send_all(stream::iter_ok((0..MESSAGES).map(|i| message::new(i))))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
            }));

            rx.for_each(|_| future::ok(()))
        }))
        .unwrap();
}

fn mpsc_unbounded() {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|cx| {
            let (tx, rx) = mpsc::unbounded();

            for _ in 0..THREADS {
                let tx = tx.clone();
                cx.spawn(future::lazy(move |_| {
                    tx.send_all(stream::iter_ok(
                        (0..MESSAGES / THREADS).map(|i| message::new(i)),
                    ))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
                }));
            }
            drop(tx);

            rx.for_each(|_| future::ok(()))
        }))
        .unwrap();
}

fn mpsc_bounded(cap: usize) {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|cx| {
            let (tx, rx) = mpsc::channel(cap);

            for _ in 0..THREADS {
                let tx = tx.clone();
                cx.spawn(future::lazy(move |_| {
                    tx.send_all(stream::iter_ok(
                        (0..MESSAGES / THREADS).map(|i| message::new(i)),
                    ))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
                }));
            }
            drop(tx);

            rx.for_each(|_| future::ok(()))
        }))
        .unwrap();
}

fn select_rx_unbounded() {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|cx| {
            let chans = (0..THREADS).map(|_| mpsc::unbounded()).collect::<Vec<_>>();

            for (tx, _) in &chans {
                let tx = tx.clone();
                cx.spawn(future::lazy(move |_| {
                    for i in 0..MESSAGES / THREADS {
                        tx.unbounded_send(message::new(i)).unwrap();
                    }
                    future::ok(())
                }));
            }

            stream::select_all(chans.into_iter().map(|(_, rx)| rx))
                .for_each(|_| future::ok(()))
                .and_then(|_| future::ok(()))
        }))
        .unwrap();
}

fn select_rx_bounded(cap: usize) {
    ThreadPool::new()
        .unwrap()
        .run(future::lazy(|cx| {
            let chans = (0..THREADS).map(|_| mpsc::channel(cap)).collect::<Vec<_>>();

            for (tx, _) in &chans {
                let tx = tx.clone();
                cx.spawn(future::lazy(move |_| {
                    tx.send_all(stream::iter_ok(
                        (0..MESSAGES / THREADS).map(|i| message::new(i)),
                    ))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
                }));
            }

            stream::select_all(chans.into_iter().map(|(_, rx)| rx))
                .for_each(|_| future::ok(()))
                .and_then(|_| future::ok(()))
        }))
        .unwrap();
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
                "Rust futures-channel",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    run!("bounded0_mpsc", mpsc_bounded(0));
    run!("bounded0_select_rx", select_rx_bounded(0));
    run!("bounded0_spsc", spsc_bounded(0));

    run!("bounded1_mpsc", mpsc_bounded(1));
    run!("bounded1_select_rx", select_rx_bounded(1));
    run!("bounded1_spsc", spsc_bounded(1));

    run!("bounded_mpsc", mpsc_bounded(MESSAGES));
    run!("bounded_select_rx", select_rx_bounded(MESSAGES));
    run!("bounded_seq", seq_bounded(MESSAGES));
    run!("bounded_spsc", spsc_bounded(MESSAGES));

    run!("unbounded_mpsc", mpsc_unbounded());
    run!("unbounded_select_rx", select_rx_unbounded());
    run!("unbounded_seq", seq_unbounded());
    run!("unbounded_spsc", spsc_unbounded());
}
