use crossbeam_skiplist::SkipSet;

#[test]
fn smoke() {
    let m = SkipSet::new();
    m.insert(1);
    m.insert(5);
    m.insert(7);
}

#[test]
fn iter() {
    let s = SkipSet::new();
    for &x in &[4, 2, 12, 8, 7, 11, 5] {
        s.insert(x);
    }

    assert_eq!(
        s.iter().map(|e| *e).collect::<Vec<_>>(),
        &[2, 4, 5, 7, 8, 11, 12]
    );

    let mut it = s.iter();
    s.remove(&2);
    assert_eq!(*it.next().unwrap(), 4);
    s.remove(&7);
    assert_eq!(*it.next().unwrap(), 5);
    s.remove(&5);
    assert_eq!(*it.next().unwrap(), 8);
    s.remove(&12);
    assert_eq!(*it.next().unwrap(), 11);
    assert!(it.next().is_none());
}
