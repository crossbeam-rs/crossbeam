extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

use std::ops::Deref;

#[test]
fn recv() {
    let (s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::unbounded::<i32>();
    let rs = [r1, r2];

    s1.send(0);
    s2.send(0);

    select! {
        recv(rs) => {}
    }

    select! {
        recv(rs) => {}
        default => panic!(),
    }

    select! {
        recv(rs) => panic!(),
        default => {}
    }
}

#[test]
fn references() {
    let (s, r) = channel::unbounded::<i32>();
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
    let (s, r) = channel::unbounded::<i32>();

    select! {
        recv(r) => 3.0
        recv(r) => loop {
            unreachable!()
        }
        recv(r) => match 7 + 3 {
            _ => unreachable!()
        }
        default => 7.
    };

    select! {
        recv(r, msg) => if msg.is_some() {
            unreachable!()
        }
        default => ()
    }

    drop(s);
}

#[test]
fn move_handles() {
    let (s, r) = channel::unbounded::<i32>();
    select! {
        recv((move || r)()) => {}
        send((move || s)(), 0) => {}
    }
}

#[test]
fn default() {
    let (s, r) = channel::bounded::<i32>(0);

    select! {
        recv(r) => panic!(),
        default => {}
    }
    select! {
        send(s, 0) => panic!(),
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
    let (_, r) = channel::unbounded::<i32>();
    select! {
        recv(r, r) => assert!(r.is_none()),
    }

    let (s, _) = channel::unbounded::<i32>();
    let s2 = s.clone();
    select! {
        send(s, 0, s) => assert_eq!(s, &s2),
    }
}

#[test]
fn handles_on_heap() {
    let (s, r) = channel::unbounded::<i32>();
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
    let (_, r) = channel::unbounded::<i32>();
    select! {
        recv(Some(&r)) => {}
    }
    select! {
        recv(Some(&r)) => {}
        recv(Some(&r)) => {}
    }

    let r: Option<channel::Receiver<u32>> = None;
    select! {
        recv(r.as_ref()) => {}
        default => {}
    }
    select! {
        recv(r.as_ref()) => {}
        recv(r.as_ref()) => {}
        default => {}
    }

    let r: Option<&&&Box<&&channel::Receiver<u32>>> = None;
    let r: Option<&channel::Receiver<u32>> = match r {
        None => None,
        Some(r) => Some(r),
    };
    select! {
        recv(r) => {}
        default => {}
    }
    select! {
        recv(r) => {}
        recv(r) => {}
        default => {}
    }
}

#[test]
fn option_sender() {
    let (s, _) = channel::unbounded::<i32>();
    select! {
        send(Some(&s), 0) => {}
        default => {}
    }
    select! {
        send(Some(&s), 0) => {}
        send(Some(&s), 0) => {}
        default => {}
    }

    let s: Option<channel::Sender<u32>> = None;
    select! {
        send(s.as_ref(), 0) => {}
        default => {}
    }
    select! {
        send(s.as_ref(), 0) => {}
        send(s.as_ref(), 0) => {}
        default => {}
    }

    let s: Option<&&&Box<&&channel::Sender<u32>>> = None;
    let s: Option<&channel::Sender<u32>> = match s {
        None => None,
        Some(s) => Some(s),
    };
    select! {
        send(s, 0) => {}
        default => {}
    }
    select! {
        send(s, 0) => {}
        send(s, 0) => {}
        default => {}
    }
}

#[test]
fn once_blocks() {
    let (s, r) = channel::unbounded::<i32>();

    let once = Box::new(());
    select! {
        send(s, 0) => drop(once),
    }

    let once = Box::new(());
    select! {
        recv(r) => drop(once),
    }

    let once1 = Box::new(());
    let once2 = Box::new(());
    select! {
        send(s, 0) => drop(once1),
        default => drop(once2),
    }

    let once1 = Box::new(());
    let once2 = Box::new(());
    select! {
        recv(r) => drop(once1),
        default => drop(once2),
    }

    let once1 = Box::new(());
    let once2 = Box::new(());
    select! {
        recv(r) => drop(once1),
        send(s, 0) => drop(once2),
    }
}

#[test]
fn once_receiver() {
    let (_, r) = channel::unbounded::<i32>();

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
    let (s, _) = channel::unbounded::<i32>();

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
fn once_receive() {

}

#[test]
fn nesting() {
    let (_, r) = channel::unbounded::<i32>();

    select! {
        recv(r) => {
            select! {
                recv(r) => {
                    select! {
                        recv(r) => {
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
    let (s, r) = channel::unbounded::<i32>();

    let v = select! {
        recv(r) => "foo".into(),
        send(s, 0) => "bar".to_owned(),
        default => "baz".to_string(),
    };
    assert_eq!(v, "bar");

    let v = select! {
        recv(r) => "foo".into(),
        default => "baz".to_string(),
    };
    assert_eq!(v, "foo");

    let v = select! {
        recv(r) => "foo".into(),
        default => "baz".to_string(),
    };
    assert_eq!(v, "baz");
}

#[test]
fn variety() {
    let (s1, r1) = channel::unbounded::<i32>();
    let (s2, r2) = channel::bounded::<String>(1);
    let (s3, r3) = channel::bounded::<()>(0);

    select! {
        recv(r1) => {}
        send(s1, 7) => {}
        recv(Some(r2)) => {}
        send(Some(s2), "foo".to_string()) => {}
        recv([&r3].iter().map(|x| *x)) => {}
        send([&s3].iter().map(|x| *x), ()) => {}
        default => {}
    }
}

#[test]
fn deref() {
    struct Sender<T>(channel::Sender<T>);
    struct Receiver<T>(channel::Receiver<T>);

    impl<T> Deref for Receiver<T> {
        type Target = channel::Receiver<T>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl<T> Deref for Sender<T> {
        type Target = channel::Sender<T>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    let (s, r) = channel::bounded::<i32>(0);
    let (s, r) = (Sender(s), Receiver(r));

    select! {
        send(s, 0) => panic!(),
        recv(r) => panic!(),
        default => {}
    }
}
