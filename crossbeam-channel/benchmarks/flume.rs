mod message;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

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
            let j = ((x as u64).wrapping_mul(n as u64) >> 32) as u32 as usize;

            v.swap(i, j);
        }
    });
}

fn seq_unbounded() {
    let (tx, rx) = flume::unbounded();

    for i in 0..MESSAGES {
        tx.send(message::new(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn seq_bounded(cap: usize) {
    let (tx, rx) = flume::bounded(cap);

    for i in 0..MESSAGES {
        tx.send(message::new(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc_unbounded() {
    let (tx, rx) = flume::unbounded();

    crossbeam::scope(|scope| {
        scope.spawn(move |_| {
            for i in 0..MESSAGES {
                tx.send(message::new(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn spsc_bounded(cap: usize) {
    let (tx, rx) = flume::bounded(cap);

    crossbeam::scope(|scope| {
        scope.spawn(move |_| {
            for i in 0..MESSAGES {
                tx.send(message::new(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_unbounded() {
    let (tx, rx) = flume::unbounded();

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            scope.spawn(move |_| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message::new(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_bounded(cap: usize) {
    let (tx, rx) = flume::bounded(cap);

    crossbeam::scope(|scope| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            scope.spawn(move |_| {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message::new(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
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
                "Rust flume",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    // run!("bounded0_mpsc", mpsc_bounded(0));
    // run!("bounded0_spsc", spsc_bounded(0));

    run!("bounded1_mpsc", mpsc_bounded(1));
    run!("bounded1_spsc", spsc_bounded(1));

    run!("bounded_mpsc", mpsc_bounded(MESSAGES));
    run!("bounded_seq", seq_bounded(MESSAGES));
    run!("bounded_spsc", spsc_bounded(MESSAGES));

    run!("unbounded_mpsc", mpsc_unbounded());
    run!("unbounded_seq", seq_unbounded());
    run!("unbounded_spsc", spsc_unbounded());
}
