use futures::channel::mpsc;
use futures::executor::{block_on, ThreadPool};
use futures::{future, stream, SinkExt, StreamExt};

mod message;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq_unbounded() {
    block_on(async {
        let (tx, rx) = mpsc::unbounded();
        for i in 0..MESSAGES {
            tx.unbounded_send(message::new(i)).unwrap();
        }
        drop(tx);

        rx.for_each(|_| future::ready(())).await
    });
}

fn seq_bounded(cap: usize) {
    let (mut tx, rx) = mpsc::channel(cap);
    block_on(async {
        for i in 0..MESSAGES {
            tx.try_send(message::new(i)).unwrap();
        }
        drop(tx);

        rx.for_each(|_| future::ready(())).await
    });
}

fn spsc_unbounded() {
    let pool = ThreadPool::new().unwrap();
    block_on(async {
        let (mut tx, rx) = mpsc::unbounded();

        pool.spawn_ok(async move {
            tx.send_all(&mut stream::iter((0..MESSAGES).map(message::new).map(Ok)))
                .await
                .unwrap()
        });

        rx.for_each(|_| future::ready(())).await
    });
}

fn spsc_bounded(cap: usize) {
    let pool = ThreadPool::new().unwrap();
    block_on(async {
        let (mut tx, rx) = mpsc::channel(cap);

        pool.spawn_ok(async move {
            tx.send_all(&mut stream::iter((0..MESSAGES).map(message::new).map(Ok)))
                .await
                .unwrap()
        });

        rx.for_each(|_| future::ready(())).await
    });
}

fn mpsc_unbounded() {
    let pool = ThreadPool::new().unwrap();
    block_on(async {
        let (tx, rx) = mpsc::unbounded();

        for _ in 0..THREADS {
            let mut tx = tx.clone();
            pool.spawn_ok(async move {
                tx.send_all(&mut stream::iter(
                    (0..MESSAGES / THREADS).map(message::new).map(Ok),
                ))
                .await
                .unwrap()
            });
        }
        drop(tx);

        rx.for_each(|_| future::ready(())).await
    });
}

fn mpsc_bounded(cap: usize) {
    let pool = ThreadPool::new().unwrap();
    block_on(async {
        let (tx, rx) = mpsc::channel(cap);

        for _ in 0..THREADS {
            let mut tx = tx.clone();
            pool.spawn_ok(async move {
                tx.send_all(&mut stream::iter(
                    (0..MESSAGES / THREADS).map(message::new).map(Ok),
                ))
                .await
                .unwrap()
            });
        }
        drop(tx);

        rx.for_each(|_| future::ready(())).await
    });
}

fn select_rx_unbounded() {
    let pool = ThreadPool::new().unwrap();
    block_on(async {
        let chans = (0..THREADS).map(|_| mpsc::unbounded()).collect::<Vec<_>>();

        for (tx, _) in &chans {
            let tx = tx.clone();
            pool.spawn_ok(async move {
                for i in 0..MESSAGES / THREADS {
                    tx.unbounded_send(message::new(i)).unwrap();
                }
            });
        }

        stream::select_all(chans.into_iter().map(|(_, rx)| rx))
            .for_each(|_| future::ready(()))
            .await
    });
}

fn select_rx_bounded(cap: usize) {
    let pool = ThreadPool::new().unwrap();
    block_on(async {
        let chans = (0..THREADS).map(|_| mpsc::channel(cap)).collect::<Vec<_>>();

        for (tx, _) in &chans {
            let mut tx = tx.clone();
            pool.spawn_ok(async move {
                tx.send_all(&mut stream::iter(
                    (0..MESSAGES / THREADS).map(message::new).map(Ok),
                ))
                .await
                .unwrap()
            });
        }

        stream::select_all(chans.into_iter().map(|(_, rx)| rx))
            .for_each(|_| future::ready(()))
            .await
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
