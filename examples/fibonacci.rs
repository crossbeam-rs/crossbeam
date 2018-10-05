//! An asynchronous fibonacci sequence generator.

#[macro_use]
extern crate crossbeam_channel as channel;

use std::thread;

fn fibonacci(fib: channel::Sender<u64>, quit: channel::Receiver<()>) {
    let (mut x, mut y) = (0, 1);
    loop {
        select! {
            send(fib, x) => {
                let tmp = x;
                x = y;
                y = tmp + y;
            }
            recv(quit) => {
                println!("quit");
                return;
            }
        }
    }
}

fn main() {
    let (fib_s, fib_r) = channel::bounded(0);
    let (quit_s, quit_r) = channel::bounded(0);

    thread::spawn(move || {
        for _ in 0..10 {
            println!("{}", fib_r.recv().unwrap());
        }
        quit_s.send(()).unwrap();
    });

    fibonacci(fib_s, quit_r);
}
