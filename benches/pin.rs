#![feature(test)]

extern crate crossbeam_epoch as epoch;
extern crate crossbeam_utils as utils;
extern crate test;

use test::Bencher;
use utils::scoped::scope;

#[bench]
fn single_pin(b: &mut Bencher) {
    b.iter(|| epoch::pin());
}

#[bench]
fn single_default_handle_pin(b: &mut Bencher) {
    b.iter(|| epoch::default_handle().pin());
}

#[bench]
fn multi_pin(b: &mut Bencher) {
    const THREADS: usize = 16;
    const STEPS: usize = 100_000;

    b.iter(|| {
        scope(|s| {
            for _ in 0..THREADS {
                s.spawn(|| {
                    for _ in 0..STEPS {
                        epoch::pin();
                    }
                });
            }
        });
    });
}
