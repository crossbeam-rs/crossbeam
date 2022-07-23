use crossbeam_deque::{Steal, Worker};
use std::thread;

mod message;

const MESSAGES: usize = 5_000_000;

fn seq() {
    let tx = Worker::new_lifo();
    let rx = tx.stealer();

    for i in 0..MESSAGES {
        tx.push(message::new(i));
    }

    for _ in 0..MESSAGES {
        match rx.steal() {
            Steal::Success(_) => {}
            Steal::Retry => panic!(),
            Steal::Empty => panic!(),
        }
    }
}

fn spsc() {
    let tx = Worker::new_lifo();
    let rx = tx.stealer();

    crossbeam::scope(|scope| {
        scope.spawn(move |_| {
            for i in 0..MESSAGES {
                tx.push(message::new(i));
            }
        });

        scope.spawn(move |_| {
            for _ in 0..MESSAGES {
                loop {
                    match rx.steal() {
                        Steal::Success(_) => break,
                        Steal::Retry | Steal::Empty => thread::yield_now(),
                    }
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
                "Rust crossbeam-deque",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    run!("unbounded_seq", seq());
    run!("unbounded_spsc", spsc());
}
