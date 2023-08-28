use std::{
    thread::{self, sleep},
    time::Duration,
};

use crossbeam_queue::StaticArrayQueue;

static QUEUE: StaticArrayQueue<u32, 10> = StaticArrayQueue::new();

fn main() {
    thread::spawn(|| loop {
        while let Some(element) = QUEUE.pop() {
            println!("Got element: {}", element);
        }
        sleep(Duration::from_millis(1));
    });

    for i in 0..10 {
        QUEUE.push(i).unwrap();
        sleep(Duration::from_millis(10));
    }
}
