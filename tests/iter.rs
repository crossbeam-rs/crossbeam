//! Tests for iteration over receivers.

extern crate crossbeam;
extern crate crossbeam_channel as channel;

#[test]
fn nested_recv_iter() {
    let (s, r) = channel::unbounded::<i32>();
    let (total_s, total_r) = channel::unbounded::<i32>();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            let mut acc = 0;
            for x in &r {
                acc += x;
            }
            total_s.send(acc);
        });

        s.send(3);
        s.send(1);
        s.send(2);
        drop(s);
        assert_eq!(total_r.recv().unwrap(), 6);
    });
}

#[test]
fn recv_iter_break() {
    let (s, r) = channel::unbounded::<i32>();
    let (count_s, count_r) = channel::unbounded();

    crossbeam::scope(|scope| {
        scope.spawn(move || {
            let mut count = 0;
            for x in &r {
                if count >= 3 {
                    break;
                } else {
                    count += x;
                }
            }
            count_s.send(count);
        });

        s.send(2);
        s.send(2);
        s.send(2);
        let _ = s.send(2);
        drop(s);
        assert_eq!(count_r.recv().unwrap(), 4);
    })
}

#[test]
fn recv_into_iter_owned() {
    let mut iter = {
        let (s, r) = channel::unbounded::<i32>();
        s.send(1);
        s.send(2);
        r.into_iter()
    };

    assert_eq!(iter.next().unwrap(), 1);
    assert_eq!(iter.next().unwrap(), 2);
    assert_eq!(iter.next().is_none(), true);
}

#[test]
fn recv_into_iter_borrowed() {
    let (s, r) = channel::unbounded::<i32>();
    s.send(1);
    s.send(2);
    drop(s);

    let mut iter = (&r).into_iter();
    assert_eq!(iter.next().unwrap(), 1);
    assert_eq!(iter.next().unwrap(), 2);
    assert_eq!(iter.next().is_none(), true);
}
