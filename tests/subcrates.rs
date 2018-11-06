//! Makes sure subcrates are properly reexported.

#[macro_use]
extern crate crossbeam;

#[test]
fn channel() {
    let (s, r) = crossbeam::channel::bounded(1);

    select! {
        send(s, 0) -> res => res.unwrap(),
        recv(r) -> res => assert!(res.is_ok()),
    }
}

#[test]
fn deque() {
    let (w, s) = crossbeam::deque::fifo();
    w.push(1);
    let _ = w.pop();
    let _ = s.steal();
}

#[test]
fn epoch() {
    crossbeam::epoch::pin();
}

#[test]
fn utils() {
    crossbeam::utils::CachePadded::new(7);

    crossbeam::scope(|scope| {
        scope.spawn(|_| ());
    }).unwrap();

    crossbeam::thread::scope(|scope| {
        scope.spawn(|_| ());
    }).unwrap();
}
