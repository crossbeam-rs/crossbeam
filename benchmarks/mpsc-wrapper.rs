extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

use shared::{message, shuffle};

mod shared;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

#[macro_use]
mod mpsc {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{RecvError, SendError};

    use channel;

    pub struct Sender<T> {
        pub inner: channel::Sender<T>,
        disconnected: channel::Receiver<()>,
        is_disconnected: Arc<AtomicBool>,
    }

    impl<T> Sender<T> {
        pub fn send(&self, t: T) -> Result<(), SendError<T>> {
            if self.is_disconnected.load(Ordering::SeqCst) {
                Err(SendError(t))
            } else {
                self.inner.send(t);
                Ok(())
            }
        }
    }

    impl<T> Clone for Sender<T> {
        fn clone(&self) -> Sender<T> {
            Sender {
                inner: self.inner.clone(),
                disconnected: self.disconnected.clone(),
                is_disconnected: self.is_disconnected.clone(),
            }
        }
    }

    pub struct SyncSender<T> {
        pub inner: channel::Sender<T>,
        disconnected: channel::Receiver<()>,
        is_disconnected: Arc<AtomicBool>,
    }

    impl<T> SyncSender<T> {
        pub fn send(&self, t: T) -> Result<(), SendError<T>> {
            if self.is_disconnected.load(Ordering::SeqCst) {
                Err(SendError(t))
            } else {
                select! {
                    send(self.inner, t) => Ok(()),
                    default => {
                        select! {
                            send(self.inner, t) => Ok(()),
                            recv(self.disconnected) => Err(SendError(t)),
                        }
                    }
                }
            }
        }
    }

    impl<T> Clone for SyncSender<T> {
        fn clone(&self) -> SyncSender<T> {
            SyncSender {
                inner: self.inner.clone(),
                disconnected: self.disconnected.clone(),
                is_disconnected: self.is_disconnected.clone(),
            }
        }
    }

    pub struct Receiver<T> {
        pub inner: channel::Receiver<T>,
        _disconnected: channel::Sender<()>,
        is_disconnected: Arc<AtomicBool>,
    }

    impl<T> Receiver<T> {
        pub fn recv(&self) -> Result<T, RecvError> {
            match self.inner.recv() {
                None => Err(RecvError),
                Some(msg) => Ok(msg),
            }
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            self.is_disconnected.store(true, Ordering::SeqCst);
        }
    }

    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let (s1, r1) = channel::unbounded();
        let (s2, r2) = channel::bounded(1);
        let is_disconnected = Arc::new(AtomicBool::new(false));

        let s = Sender {
            inner: s1,
            disconnected: r2,
            is_disconnected: is_disconnected.clone(),
        };
        let r = Receiver {
            inner: r1,
            _disconnected: s2,
            is_disconnected,
        };
        (s, r)
    }

    pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
        let (s1, r1) = channel::bounded(bound);
        let (s2, r2) = channel::bounded(1);
        let is_disconnected = Arc::new(AtomicBool::new(false));

        let s = SyncSender {
            inner: s1,
            disconnected: r2,
            is_disconnected: is_disconnected.clone(),
        };
        let r = Receiver {
            inner: r1,
            _disconnected: s2,
            is_disconnected,
        };
        (s, r)
    }

    macro_rules! mpsc_select {
        (
            $($name:pat = $rx:ident.$meth:ident() => $code:expr),+
        ) => ({
            select! {
                $(
                    $meth(($rx).inner, msg) => {
                        let $name = match msg {
                            None => Err(::std::sync::mpsc::RecvError),
                            Some(msg) => Ok(msg),
                        };
                        $code
                    }
                )+
            }
        })
    }
}

fn seq_async() {
    let (tx, rx) = mpsc::channel();

    for i in 0..MESSAGES {
        tx.send(message(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn seq_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel(cap);

    for i in 0..MESSAGES {
        tx.send(message(i)).unwrap();
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc_async() {
    let (tx, rx) = mpsc::channel();

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(message(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn spsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel(cap);

    crossbeam::scope(|s| {
        s.spawn(move || {
            for i in 0..MESSAGES {
                tx.send(message(i)).unwrap();
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_async() {
    let (tx, rx) = mpsc::channel();

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn mpsc_sync(cap: usize) {
    let (tx, rx) = mpsc::sync_channel(cap);

    crossbeam::scope(|s| {
        for _ in 0..THREADS {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

fn select_rx_async() {
    assert_eq!(THREADS, 4);
    let mut chans = (0..THREADS).map(|_| mpsc::channel()).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            shuffle(&mut chans);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            mpsc_select! {
                m = rx0.recv() => assert!(m.is_ok()),
                m = rx1.recv() => assert!(m.is_ok()),
                m = rx2.recv() => assert!(m.is_ok()),
                m = rx3.recv() => assert!(m.is_ok())
            }
        }
    });
}

fn select_rx_sync(cap: usize) {
    assert_eq!(THREADS, 4);
    let mut chans = (0..THREADS).map(|_| mpsc::sync_channel(cap)).collect::<Vec<_>>();

    crossbeam::scope(|s| {
        for &(ref tx, _) in &chans {
            let tx = tx.clone();
            s.spawn(move || {
                for i in 0..MESSAGES / THREADS {
                    tx.send(message(i)).unwrap();
                }
            });
        }

        for _ in 0..MESSAGES {
            shuffle(&mut chans);
            let rx0 = &chans[0].1;
            let rx1 = &chans[1].1;
            let rx2 = &chans[2].1;
            let rx3 = &chans[3].1;

            mpsc_select! {
                m = rx0.recv() => assert!(m.is_ok()),
                m = rx1.recv() => assert!(m.is_ok()),
                m = rx2.recv() => assert!(m.is_ok()),
                m = rx3.recv() => assert!(m.is_ok())
            }
        }
    });
}

fn main() {
    macro_rules! run {
        ($name:expr, $f:expr) => {
            let now = ::std::time::Instant::now();
            $f;
            let elapsed = now.elapsed();
            println!(
                "{:25} {:15} {:7.3} sec",
                $name,
                "Rust mpsc-wrapper",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("bounded0_mpsc", mpsc_sync(0));
    run!("bounded0_select_rx", select_rx_sync(0));
    run!("bounded0_spsc", spsc_sync(0));

    run!("bounded1_mpsc", mpsc_sync(1));
    run!("bounded1_select_rx", select_rx_sync(1));
    run!("bounded1_spsc", spsc_sync(1));

    run!("bounded_mpsc", mpsc_sync(MESSAGES));
    run!("bounded_select_rx", select_rx_sync(MESSAGES));
    run!("bounded_seq", seq_sync(MESSAGES));
    run!("bounded_spsc", spsc_sync(MESSAGES));

    run!("unbounded_mpsc", mpsc_async());
    run!("unbounded_select_rx", select_rx_async());
    run!("unbounded_seq", seq_async());
    run!("unbounded_spsc", spsc_async());
}
