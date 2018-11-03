//! Tests for the tick channel flavor.

// TODO: update this file

extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;
extern crate rand;

use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{never, unbounded, TryRecvError};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn foo() {
    let (s, r) = unbounded::<i32>();
    let r = Some(&r);

    select! {
        recv(r.unwrap_or(&never())) -> _ => panic!(),
        default => {}
    }
}

#[test]
fn bar() {
    let (s, r) = unbounded::<i32>();
    let r = Some(r);

    select! {
        recv(r.unwrap_or(never())) -> _ => panic!(),
        default => {}
    }
}

// #[test]
// fn fire() {
//     let start = Instant::now();
//     let r = tick(ms(50));
//
//     assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
//     thread::sleep(ms(100));
//
//     let fired = r.try_recv().unwrap();
//     assert!(start < fired);
//     assert!(fired - start >= ms(50));
//
//     let now = Instant::now();
//     assert!(fired < now);
//     assert!(now - fired >= ms(50));
//
//     assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
//
//     select! {
//         recv(r) -> _ => panic!(),
//         default => {}
//     }
//
//     select! {
//         recv(r) -> _ => {}
//         recv(tick(ms(200))) -> _ => panic!(),
//     }
// }

#[test]
fn capacity() {
    let r = never::<i32>();
    assert_eq!(r.capacity(), Some(1));
}

#[test]
fn len_empty_full() {
    let r = never::<i32>();
    assert_eq!(r.capacity(), Some(1));
    assert_eq!(r.len(), 0);
    assert_eq!(r.is_empty(), true);
    assert_eq!(r.is_full(), false);
}

// #[test]
// fn try_recv() {
//     let r = tick(ms(200));
//     assert!(r.try_recv().is_err());
//
//     thread::sleep(ms(100));
//     assert!(r.try_recv().is_err());
//
//     thread::sleep(ms(200));
//     assert!(r.try_recv().is_ok());
//     assert!(r.try_recv().is_err());
//
//     thread::sleep(ms(200));
//     assert!(r.try_recv().is_ok());
//     assert!(r.try_recv().is_err());
// }
//
// #[test]
// fn recv() {
//     let start = Instant::now();
//     let r = tick(ms(50));
//
//     let fired = r.recv().unwrap();
//     assert!(start < fired);
//     assert!(fired - start >= ms(50));
//
//     let now = Instant::now();
//     assert!(fired < now);
//     assert!(now - fired < fired - start);
//
//     assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
// }
//
// #[test]
// fn recv_timeout() {
//     let start = Instant::now();
//     let r = tick(ms(200));
//
//     assert!(r.recv_timeout(ms(100)).is_err());
//     let now = Instant::now();
//     assert!(now - start >= ms(100));
//     assert!(now - start <= ms(150));
//
//     let fired = r.recv_timeout(ms(200)).unwrap();
//     assert!(fired - start >= ms(200));
//     assert!(fired - start <= ms(250));
//
//     assert!(r.recv_timeout(ms(100)).is_err());
//     let now = Instant::now();
//     assert!(now - start >= ms(300));
//     assert!(now - start <= ms(350));
//
//     let fired = r.recv_timeout(ms(200)).unwrap();
//     assert!(fired - start >= ms(400));
//     assert!(fired - start <= ms(450));
// }
