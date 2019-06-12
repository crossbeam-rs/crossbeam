use std::time::Duration;

use crossbeam_channel::{after, bounded, never, tick, unbounded};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn after_identical_to() {
    let r = after(ms(50));

    let r2 = r.clone();
    assert!(r.identical_to(&r2));

    let r3 = after(ms(50));
    assert!(!r.identical_to(&r3));
    assert!(!r2.identical_to(&r3));

    let r4 = after(ms(100));
    assert!(!r.identical_to(&r4));
    assert!(!r2.identical_to(&r4));
}

#[test]
fn array_identical_to() {
    let (s, r) = bounded::<usize>(1);

    let s2 = s.clone();
    assert!(s.identical_to(&s2));

    let r2 = r.clone();
    assert!(r.identical_to(&r2));

    let (s3, r3) = bounded::<usize>(1);
    assert!(!s.identical_to(&s3));
    assert!(!s2.identical_to(&s3));
    assert!(!r.identical_to(&r3));
    assert!(!r2.identical_to(&r3));
}

#[test]
fn list_identical_to() {
    let (s, r) = unbounded::<usize>();

    let s2 = s.clone();
    assert!(s.identical_to(&s2));

    let r2 = r.clone();
    assert!(r.identical_to(&r2));

    let (s3, r3) = unbounded::<usize>();
    assert!(!s.identical_to(&s3));
    assert!(!s2.identical_to(&s3));
    assert!(!r.identical_to(&r3));
    assert!(!r2.identical_to(&r3));
}

#[test]
fn never_identical_to() {
    let r = never::<usize>();

    let r2 = r.clone();
    assert!(r.identical_to(&r2));

    // Never channel are always equal to one another.
    let r3 = never::<usize>();
    assert!(r.identical_to(&r3));
    assert!(r2.identical_to(&r3));
}

#[test]
fn tick_identical_to() {
    let r = tick(ms(50));

    let r2 = r.clone();
    assert!(r.identical_to(&r2));

    let r3 = tick(ms(50));
    assert!(!r.identical_to(&r3));
    assert!(!r2.identical_to(&r3));

    let r4 = tick(ms(100));
    assert!(!r.identical_to(&r4));
    assert!(!r2.identical_to(&r4));
}

#[test]
fn zero_identical_to() {
    let (s, r) = bounded::<usize>(0);

    let s2 = s.clone();
    assert!(s.identical_to(&s2));

    let r2 = r.clone();
    assert!(r.identical_to(&r2));

    let (s3, r3) = bounded::<usize>(0);
    assert!(!s.identical_to(&s3));
    assert!(!s2.identical_to(&s3));
    assert!(!r.identical_to(&r3));
    assert!(!r2.identical_to(&r3));
}

#[test]
fn different_flavours_identical_to() {
    let (s1, r1) = bounded::<usize>(0);
    let (s2, r2) = unbounded::<usize>();
    assert!(!s1.identical_to(&s2));
    assert!(!r1.identical_to(&r2));
}
