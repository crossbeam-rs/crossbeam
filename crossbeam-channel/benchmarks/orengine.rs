use orengine::{stop_all_executors, Executor};
use orengine::sync::{global_scope, Channel};

mod message;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 5;

fn new<T>(cap: Option<usize>) -> Channel<T> {
    match cap {
        None => Channel::unbounded(),
        Some(cap) => Channel::bounded(cap),
    }
}

fn seq(cap: Option<usize>) {
    let cfg = orengine::runtime::Config::default()
        .disable_io_worker()
        .disable_work_sharing()
        .set_numbers_of_thread_workers(0);
    Executor::init_with_config(cfg).run_and_block_on_global(async move {
        let chan = new(cap);
        for i in 0..MESSAGES {
            let _ = chan.send(message::new(i)).await;
        }

        for _ in 0..MESSAGES {
            let _ = chan.recv().await;
        }
    }).expect("failed to run");
}

fn spsc(cap: Option<usize>) {
    let cfg = orengine::runtime::Config::default()
        .disable_io_worker()
        .disable_work_sharing()
        .set_numbers_of_thread_workers(0);
    Executor::init_with_config(cfg).run_and_block_on_global(async move {
        let chan = new(cap);

        global_scope(|scope| async {
            scope.spawn(async {
                for i in 0..MESSAGES {
                    let _ = chan.send(message::new(i)).await;
                }
            });

            for _ in 0..MESSAGES {
                let _ = chan.recv().await;
            }
        }).await;
    }).expect("failed to run");
}

fn mpsc(cap: Option<usize>) {
    let cfg = orengine::runtime::Config::default()
        .set_work_sharing_level(0)
        .set_numbers_of_thread_workers(1);
    let chan = new(cap);
    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|_| {
                Executor::init_with_config(cfg).run_with_global_future(async {
                    for i in 0..MESSAGES / THREADS {
                        let _ = chan.send(message::new(i)).await;
                    }
                })
            });
        }

        Executor::init_with_config(cfg).run_with_global_future(async {
            for _ in 0..MESSAGES {
                let _ = chan.recv().await;
            }
            stop_all_executors();
        });
    }).expect("scope failed");
}

fn mpmc(cap: Option<usize>) {
    let cfg = orengine::runtime::Config::default()
        .set_work_sharing_level(1)
        .set_numbers_of_thread_workers(0);
    let chan = new(cap);
    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|_| {
                Executor::init_with_config(cfg).run_with_global_future(async {
                    for i in 0..MESSAGES / THREADS {
                        let _ = chan.send(message::new(i)).await;
                    }
                });
            });
        }

        for _ in 0..THREADS {
            s.spawn(|_| {
                Executor::init_with_config(cfg).run_with_global_future(async {
                    for _ in 0..MESSAGES / THREADS {
                        let _ = chan.recv().await;
                    }
                    stop_all_executors();
                });
            });
        }
    }).expect("scope failed");
}

fn main() {
    macro_rules! run {
        ($name:expr, $f:expr) => {
            let mut total = 0;
            for _ in 0..15 {
                let now = ::std::time::Instant::now();
                $f;
                let elapsed = now.elapsed();
                total += elapsed.as_nanos();
            }

            let avg = total as f64 / 15.0;
            let elapsed = ::std::time::Duration::from_nanos(avg.round() as u64);
            println!(
                "{:25} {:15} {:7.3} sec",
                $name,
                "Rust orengine",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        };
    }

    run!("bounded0_mpmc", mpmc(Some(0)));
    run!("bounded0_mpsc", mpsc(Some(0)));
    run!("bounded0_spsc", spsc(Some(0)));

    run!("bounded1_mpmc", mpmc(Some(1)));
    run!("bounded1_mpsc", mpsc(Some(1)));
    run!("bounded1_spsc", spsc(Some(1)));

    run!("bounded_mpmc", mpmc(Some(MESSAGES)));
    run!("bounded_mpsc", mpsc(Some(MESSAGES)));
    run!("bounded_seq", seq(Some(MESSAGES)));
    run!("bounded_spsc", spsc(Some(MESSAGES)));

    run!("unbounded_mpmc", mpmc(None));
    run!("unbounded_mpsc", mpsc(None));
    run!("unbounded_seq", seq(None));
    run!("unbounded_spsc", spsc(None));
}
