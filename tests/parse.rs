extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;

use std::time::{Duration, Instant};

use crossbeam_channel::unbounded;

#[test]
fn references() {
    let (s, r) = unbounded::<i32>();
    select! {
        send(s, 0) => {}
        recv(r) => {}
    }

    select! {
        send(&&&&s, 0) => {}
        recv(&&&&r) => {}
    }

    select! {
        send([&s].iter().map(|x| *x), 0) => {}
        recv([&r].iter().map(|x| *x)) => {}
    }

    let ss = &&&&[s];
    let rr = &&&&[r];
    select! {
        send(&&&ss.iter(), 0) => {}
        recv(&&&&rr.iter()) => {}
    }
    // TODO: refs in the multi case?
}

#[test]
fn default_instant() {
    select! {
        default(Instant::now()) => {}
    }
    select! {
        default(&&&&Instant::now()) => {}
    }

    let instant = Instant::now();
    select! {
        default(instant) => {}
    }
    select! {
        default(&&&&instant) => {}
    }
}

#[test]
fn default_duration() {
    select! {
        default(Duration::from_secs(0)) => {}
    }
    select! {
        default(&&&&Duration::from_secs(0)) => {}
    }

    let duration = Instant::now();
    select! {
        default(duration) => {}
    }
    select! {
        default(&&&&duration) => {}
    }
}

// TODO: for loop where only odd/even receivers from an iterator are selected
