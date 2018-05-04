extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;

use std::any::Any;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

#[test]
fn references() {
    let (s, r) = unbounded::<i32>();
    select! {
        send(s, 0) => {}
        recv(r, _) => {}
    }

    select! {
        send(&&&&s, 0) => {}
        recv(&&&&r, _) => {}
    }

    select! {
        send([&s].iter().map(|x| *x), 0, _) => {}
        recv([&r].iter().map(|x| *x), _) => {}
    }

    let ss = &&&&[s];
    let rr = &&&&[r];
    select! {
        // TODO send(&&&ss.iter(), 0, _) => {}
        recv(&&&&rr.iter(), _) => {}
    }
    // TODO: refs in the multi case?
}

// TODO: for loop where only odd/even receivers from an iterator are selected
