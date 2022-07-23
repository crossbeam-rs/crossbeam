use bus::Bus;

mod message;

const MESSAGES: usize = 5_000_000;

fn seq(cap: usize) {
    let mut tx = Bus::new(cap);
    let mut rx = tx.add_rx();

    for i in 0..MESSAGES {
        tx.broadcast(message::new(i));
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc(cap: usize) {
    let mut tx = Bus::new(cap);
    let mut rx = tx.add_rx();

    crossbeam::scope(|scope| {
        scope.spawn(|_| {
            for i in 0..MESSAGES {
                tx.broadcast(message::new(i));
            }
        });

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
                "Rust bus",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    run!("bounded1_spsc", spsc(1));

    run!("bounded_seq", seq(MESSAGES));
    run!("bounded_spsc", spsc(MESSAGES));
}
