#![feature(test)]

extern crate test;

use core::sync::atomic::Ordering;
use crossbeam_epoch::arc::{Arc, Atomic};
use crossbeam_epoch::pin;
use test::Bencher;

#[bench]
fn arc_new(b: &mut Bencher) {
    b.iter(|| Arc::new(123));
}

#[bench]
fn arc_load(b: &mut Bencher) {
    let a = Atomic::new(Arc::new(123));
    let guard = pin();
    b.iter(|| {
        unsafe { a.load(Ordering::Acquire, &guard) };
    });
}
