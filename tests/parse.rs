//! Tests for the `select!` macro parser.
//!
//! These tests make sure that all possible invocations of `select!` actually compile.

#![deny(unsafe_code)]

extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel;

use std::ops::Deref;

use crossbeam_channel as cc;
use crossbeam_channel::{bounded, unbounded};

#[test]
fn references() {
    let (s, r) = unbounded::<i32>();
    select! {
        send(s, 0) -> _ => {}
        recv(r) -> _ => {}
    }
    select! {
        send(&&&&s, 0) -> _ => {}
        recv(&&&&r) -> _ => {}
    }
}

#[test]
fn blocks() {
    let (s, r) = unbounded::<i32>();

    select! {
        recv(r) -> _ => 3.0,
        recv(r) -> _ => loop {
            unreachable!()
        },
        recv(r) -> _ => match 7 + 3 {
            _ => unreachable!()
        },
        default => 7.
    };

    select! {
        recv(r) -> msg => if msg.is_ok() {
            unreachable!()
        },
        default => ()
    }

    drop(s);
}

#[test]
fn move_handles() {
    let (s, r) = unbounded::<i32>();
    select! {
        recv((move || r)()) -> _ => {}
        send((move || s)(), 0) -> _ => {}
    }
}

#[test]
fn infer_types() {
    let (s, r) = unbounded();
    select! {
        recv(r) -> _ => {}
        default => {}
    }
    s.send(()).unwrap();

    let (s, r) = unbounded();
    select! {
        send(s, ()) -> _ => {}
    }
    r.recv().unwrap();
}

#[test]
fn default() {
    let (s, r) = bounded::<i32>(0);

    select! {
        recv(r) -> _ => panic!(),
        default => {}
    }
    select! {
        send(s, 0) -> _ => panic!(),
        default() => {}
    }
    select! {
        default => {}
    }
    select! {
        default() => {}
    }
}

#[test]
fn same_variable_name() {
    let (_, r) = unbounded::<i32>();
    select! {
        recv(r) -> r => assert!(r.is_err()),
    }
}

#[test]
fn handles_on_heap() {
    let (s, r) = unbounded::<i32>();
    let (s, r) = (Box::new(s), Box::new(r));

    select! {
        send(*s, 0) -> _ => {}
        recv(*r) -> _ => {}
        default => {}
    }

    drop(s);
    drop(r);
}

#[test]
fn once_blocks() {
    let (s, r) = unbounded::<i32>();

    let once = Box::new(());
    select! {
        send(s, 0) -> _ => drop(once),
    }

    let once = Box::new(());
    select! {
        recv(r) -> _ => drop(once),
    }

    let once1 = Box::new(());
    let once2 = Box::new(());
    select! {
        send(s, 0) -> _ => drop(once1),
        default => drop(once2),
    }

    let once1 = Box::new(());
    let once2 = Box::new(());
    select! {
        recv(r) -> _ => drop(once1),
        default => drop(once2),
    }

    let once1 = Box::new(());
    let once2 = Box::new(());
    select! {
        recv(r) -> _ => drop(once1),
        send(s, 0) -> _ => drop(once2),
    }
}

#[test]
fn once_receiver() {
    let (_, r) = unbounded::<i32>();

    let once = Box::new(());
    let get = move || {
        drop(once);
        r
    };

    select! {
        recv(get()) -> _ => {}
    }
}

#[test]
fn once_sender() {
    let (s, _) = unbounded::<i32>();

    let once = Box::new(());
    let get = move || {
        drop(once);
        s
    };

    select! {
        send(get(), 5) -> _ => {}
    }
}

#[test]
fn nesting() {
    let (_, r) = unbounded::<i32>();

    select! {
        recv(r) -> _ => {
            select! {
                recv(r) -> _ => {
                    select! {
                        recv(r) -> _ => {
                            select! {
                                default => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn evaluate() {
    let (s, r) = unbounded::<i32>();

    let v = select! {
        recv(r) -> _ => "foo".into(),
        send(s, 0) -> _ => "bar".to_owned(),
        default => "baz".to_string(),
    };
    assert_eq!(v, "bar");

    let v = select! {
        recv(r) -> _ => "foo".into(),
        default => "baz".to_string(),
    };
    assert_eq!(v, "foo");

    let v = select! {
        recv(r) -> _ => "foo".into(),
        default => "baz".to_string(),
    };
    assert_eq!(v, "baz");
}

#[test]
fn deref() {
    struct Sender<T>(cc::Sender<T>);
    struct Receiver<T>(cc::Receiver<T>);

    impl<T> Deref for Receiver<T> {
        type Target = cc::Receiver<T>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl<T> Deref for Sender<T> {
        type Target = cc::Sender<T>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    let (s, r) = bounded::<i32>(0);
    let (s, r) = (Sender(s), Receiver(r));

    select! {
        send(s, 0) -> _ => panic!(),
        recv(r) -> _ => panic!(),
        default => {}
    }
}
