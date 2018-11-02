//! Prints the elapsed time every 1 second and quits on Ctrl+C.

#[macro_use]
extern crate crossbeam_channel;
extern crate signal_hook;

use std::io;
use std::time::{Duration, Instant};
use std::thread;
use crossbeam_channel::{bounded, tick, Receiver};

fn sigint_notifier() -> io::Result<Receiver<()>> {
    let (s, r) = bounded(100);
    let signals = signal_hook::iterator::Signals::new(&[signal_hook::SIGINT])?;

    thread::spawn(move || {
        for _ in signals.forever() {
            if s.send(()).is_err() {
                break;
            }
        }
    });

    Ok(r)
}

fn main() {
    let ctrl_c = sigint_notifier().unwrap();
    let update = tick(Duration::from_secs(1));
    let now = Instant::now();

    loop {
        select! {
            recv(update) -> _ => {
                println!("Elapsed time: {:?}", now.elapsed());
            }
            recv(ctrl_c) -> _ => {
                println!("Goodbye!");
                break;
            }
        }
    }
}
