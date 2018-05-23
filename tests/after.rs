extern crate crossbeam;
// #[macro_use]
extern crate crossbeam_channel as channel;
extern crate rand;

use std::thread;
use std::time::Duration;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn smoke() {
    let r = channel::after(ms(0));
    thread::sleep(ms(10));
    assert!(r.recv().is_some());
}
