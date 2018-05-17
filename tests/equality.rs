extern crate crossbeam;
extern crate crossbeam_channel as channel;

#[test]
fn sender() {
    let (s1, _) = channel::unbounded::<()>();
    let s2 = s1.clone();
    let (s3, _) = channel::unbounded();

    assert_eq!(s1, s2);
    assert_ne!(s1, s3);
    assert_ne!(s2, s3);
    assert_eq!(s3, s3);
}

#[test]
fn receiver() {
    let (_, r1) = channel::unbounded::<()>();
    let r2 = r1.clone();
    let (_, r3) = channel::unbounded();

    assert_eq!(r1, r2);
    assert_ne!(r1, r3);
    assert_ne!(r2, r3);
    assert_eq!(r3, r3);
}

#[test]
fn sender_and_receiver() {
    let (s1, r1) = channel::unbounded::<()>();
    let s2 = s1.clone();
    let r2 = r1.clone();
    let (s3, r3) = channel::unbounded();

    assert_eq!(s1, r2);
    assert_eq!(r1, s2);

    assert_ne!(s1, r3);
    assert_ne!(r1, s3);

    assert_ne!(s2, r3);
    assert_ne!(r2, s3);

    assert_eq!(s3, r3);
    assert_eq!(r3, s3);
}
