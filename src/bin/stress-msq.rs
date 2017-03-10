extern crate crossbeam;

use crossbeam::sync::MsQueue;

use std::sync::Arc;
use std::thread;

const DUP: usize = 4;
const THREADS: u32 = 2;
const COUNT: u64 = 100000;

fn main() {
    let mut v = Vec::new();
    for _i in 0..DUP {
        let q = Arc::new(MsQueue::new());
        let qs = q.clone();

        v.push(thread::spawn(move || {
            for i in 1..COUNT { qs.queue(i) }
        }));

        for _i in 0..THREADS {
            let qr = q.clone();
            v.push(thread::spawn(move || {
                let mut cur: u64 = 0;
                for _j in 0..COUNT {
                    if let Some(new) = qr.dequeue() {
                        assert!(new > cur);
                        cur = new;
                    }
                }
            }));
        }
    }

    for i in v {
        i.join().unwrap();
    }
}
