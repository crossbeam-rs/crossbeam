extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as chan;

use std::time::{Duration, Instant};

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn references() {
    let (s, r) = chan::unbounded::<i32>();
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
        send(ss.iter(), 0) => {}
        recv(rr.iter()) => {}
    }
    select! {
        send(&&&&ss.iter(), 0) => {}
        recv(&&&&rr.iter()) => {}
    }
}

#[test]
fn blocks() {
    let (s, r) = chan::unbounded::<i32>();

    select! {
        recv(r) => 3.0
        recv(r) => loop {
            unreachable!()
        }
        recv(r) => match 7 + 3 {
            _ => unreachable!()
        }
        default() => 7.
    };

    select! {
        recv(r, msg) => if msg.is_some() {
            unreachable!()
        }
        default() => ()
    }

    drop(s);
}

#[test]
fn default_instant() {
    select! {
        default(Instant::now()) => {}
    }
    select! {
        default(&&&&Instant::now()) => {}
    }
    select! {
        default(Some(Instant::now())) => {}
    }
    select! {
        default(&&&&Some(Instant::now())) => {}
    }

    let instant = Instant::now();
    select! {
        default(instant) => {}
    }
    select! {
        default(&&&&instant) => {}
    }
    select! {
        default(Some(instant)) => {}
    }
    select! {
        default(&&&&Some(instant)) => {}
    }
}

#[test]
fn default_duration() {
    select! {
        default(ms(0)) => {}
    }
    select! {
        default(&&&&ms(0)) => {}
    }
    select! {
        default(Some(ms(0))) => {}
    }
    select! {
        default(&&&&Some(ms(0))) => {}
    }

    let duration = ms(0);
    select! {
        default(duration) => {}
    }
    select! {
        default(&&&&duration) => {}
    }
    select! {
        default(Some(duration)) => {}
    }
    select! {
        default(&&&&Some(duration)) => {}
    }
}

#[test]
fn same_variable_name() {
    let (_, r) = chan::unbounded::<i32>();
    select! {
        recv(r, r) => assert!(r.is_none()),
    }

    let (s, _) = chan::unbounded::<i32>();
    let s2 = s.clone();
    select! {
        send(s, 0, s) => assert_eq!(s, &s2),
    }
}

#[test]
fn handles_on_heap() {
    let (s, r) = chan::unbounded::<i32>();
    let (s, r) = (Box::new(s), Box::new(r));

    select! {
        send(*s, 0) => {}
        recv(*r) => {}
        default => {}
    }

    drop(s);
    drop(r);
}

#[test]
fn option_receiver() {
    let (_, r) = chan::unbounded::<i32>();
    select! {
        recv(Some(&r)) => {}
    }

    let r: Option<chan::Receiver<u32>> = None;
    select! {
        recv(r.as_ref()) => {}
        default => {}
    }

    let r: Option<&&&Box<&&chan::Receiver<u32>>> = None;
    let r: Option<&chan::Receiver<u32>> = match r {
        None => None,
        Some(r) => Some(r),
    };
    select! {
        recv(r) => {}
        default => {}
    }
    // TODO: test with multiple cases
}

#[test]
fn option_sender() {
    let (s, _) = chan::unbounded::<i32>();
    select! {
        send(Some(&s), 0) => {}
        default => {}
    }

    let s: Option<chan::Sender<u32>> = None;
    select! {
        send(s.as_ref(), 0) => {}
        default => {}
    }

    let s: Option<&&&Box<&&chan::Sender<u32>>> = None;
    let s: Option<&chan::Sender<u32>> = match s {
        None => None,
        Some(s) => Some(s),
    };
    select! {
        send(s, 0) => {}
        default => {}
    }
    // TODO: test with multiple cases
}

#[test]
fn once_receiver() {
    let (_, r) = chan::unbounded::<i32>();

    let once = Box::new(());
    let get = move || {
        drop(once);
        r
    };

    select! {
        recv(get()) => {}
    }
}

#[test]
fn once_sender() {
    let (s, _) = chan::unbounded::<i32>();

    let once = Box::new(());
    let get = move || {
        drop(once);
        s
    };

    select! {
        send(get(), 5) => {}
    }
}

#[test]
fn once_timeout() {
    let once = Box::new(());
    let get = move || {
        drop(once);
        ms(10)
    };

    select! {
        default(get()) => {}
    }
}

// TODO: also fn once_deadline

// TODO: for loop where only odd/even receivers from an iterator are selected
