extern crate crossbeam;
extern crate futures;

use futures::channel::mpsc;
use futures::executor::ThreadPool;
use futures::prelude::*;
use futures::{SinkExt, StreamExt, future, stream};

use shared::message;

mod shared;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq_unbounded() {
    ThreadPool::new().unwrap().run(future::lazy(|_| {
        let (tx, rx) = mpsc::unbounded();
        for i in 0..MESSAGES {
            tx.unbounded_send(message(i)).unwrap();
        }
        drop(tx);

        rx.for_each(|_| future::ok(()))
    })).unwrap();
}

fn seq_bounded(cap: usize) {
    let (mut tx, rx) = mpsc::channel(cap);
    ThreadPool::new().unwrap().run(future::lazy(|_| {
        for i in 0..MESSAGES {
            tx.try_send(message(i)).unwrap();
        }
        drop(tx);

        rx.for_each(|_| future::ok(()))
    })).unwrap();
}

fn spsc_unbounded() {
    ThreadPool::new().unwrap().run(future::lazy(|cx| {
        let (tx, rx) = mpsc::unbounded();

        cx.spawn(future::lazy(move |_| {
            tx.send_all(stream::iter_ok((0..MESSAGES).map(|i| message(i))))
                .map_err(|_| panic!())
                .and_then(|_| future::ok(()))
        }));

        rx.for_each(|_| future::ok(()))
    })).unwrap();
}

fn spsc_bounded(cap: usize) {
    ThreadPool::new().unwrap().run(future::lazy(|cx| {
        let (tx, rx) = mpsc::channel(cap);

        cx.spawn(future::lazy(move |_| {
            tx.send_all(stream::iter_ok((0..MESSAGES).map(|i| message(i))))
                .map_err(|_| panic!())
                .and_then(|_| future::ok(()))
        }));

        rx.for_each(|_| future::ok(()))
    })).unwrap();
}

fn mpsc_unbounded() {
    ThreadPool::new().unwrap().run(future::lazy(|cx| {
        let (tx, rx) = mpsc::unbounded();

        for _ in 0..THREADS {
            let tx = tx.clone();
            cx.spawn(future::lazy(move |_| {
                tx.send_all(stream::iter_ok((0..MESSAGES / THREADS).map(|i| message(i))))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
            }));
        }
        drop(tx);

        rx.for_each(|_| future::ok(()))
    })).unwrap();
}

fn mpsc_bounded(cap: usize) {
    ThreadPool::new().unwrap().run(future::lazy(|cx| {
        let (tx, rx) = mpsc::channel(cap);

        for _ in 0..THREADS {
            let tx = tx.clone();
            cx.spawn(future::lazy(move |_| {
                tx.send_all(stream::iter_ok((0..MESSAGES / THREADS).map(|i| message(i))))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
            }));
        }
        drop(tx);

        rx.for_each(|_| future::ok(()))
    })).unwrap();
}

fn select_rx_unbounded() {
    ThreadPool::new().unwrap().run(future::lazy(|cx| {
        let chans = (0..THREADS)
            .map(|_| mpsc::unbounded())
            .collect::<Vec<_>>();

        for (tx, _) in &chans {
            let tx = tx.clone();
            cx.spawn(future::lazy(move |_| {
                for i in 0..MESSAGES / THREADS {
                    tx.unbounded_send(message(i)).unwrap();
                }
                future::ok(())
            }));
        }

        select_all::select_all(chans.into_iter().map(|(_, rx)| rx))
            .for_each(|_| future::ok(()))
            .and_then(|_| future::ok(()))
    })).unwrap();
}

fn select_rx_bounded(cap: usize) {
    ThreadPool::new().unwrap().run(future::lazy(|cx| {
        let chans = (0..THREADS)
            .map(|_| mpsc::channel(cap))
            .collect::<Vec<_>>();

        for (tx, _) in &chans {
            let tx = tx.clone();
            cx.spawn(future::lazy(move |_| {
                tx.send_all(stream::iter_ok((0..MESSAGES / THREADS).map(|i| message(i))))
                    .map_err(|_| panic!())
                    .and_then(|_| future::ok(()))
            }));
        }

        select_all::select_all(chans.into_iter().map(|(_, rx)| rx))
            .for_each(|_| future::ok(()))
            .and_then(|_| future::ok(()))
    })).unwrap();
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
                "Rust futures-channel",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    run!("bounded0_mpsc", mpsc_bounded(0));
    run!("bounded0_select_rx", select_rx_bounded(0));
    run!("bounded0_spsc", spsc_bounded(0));

    run!("bounded1_mpsc", mpsc_bounded(1));
    run!("bounded1_select_rx", select_rx_bounded(1));
    run!("bounded1_spsc", spsc_bounded(1));

    run!("bounded_mpsc", mpsc_bounded(MESSAGES));
    run!("bounded_select_rx", select_rx_bounded(MESSAGES));
    run!("bounded_seq", seq_bounded(MESSAGES));
    run!("bounded_spsc", spsc_bounded(MESSAGES));

    run!("unbounded_mpsc", mpsc_unbounded());
    run!("unbounded_select_rx", select_rx_unbounded());
    run!("unbounded_seq", seq_unbounded());
    run!("unbounded_spsc", spsc_unbounded());
}

/// The only difference from the original is this bugfix:
/// https://github.com/rust-lang-nursery/futures-rs/pull/1035
mod select_all {
    //! An unbounded set of streams

    use std::fmt::{self, Debug};

    use futures::{Async, Poll, Stream};
    use futures::task;

    use futures::stream::{StreamExt, StreamFuture, FuturesUnordered};

    /// An unbounded set of streams
    ///
    /// This "combinator" provides the ability to maintain a set of streams
    /// and drive them all to completion.
    ///
    /// Streams are pushed into this set and their realized values are
    /// yielded as they become ready. Streams will only be polled when they
    /// generate notifications. This allows to coordinate a large number of streams.
    ///
    /// Note that you can create a ready-made `SelectAll` via the
    /// `select_all` function in the `stream` module, or you can start with an
    /// empty set with the `SelectAll::new` constructor.
    #[must_use = "streams do nothing unless polled"]
    pub struct SelectAll<S> {
        inner: FuturesUnordered<StreamFuture<S>>,
    }

    impl<T: Debug> Debug for SelectAll<T> {
        fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
            write!(fmt, "SelectAll {{ ... }}")
        }
    }

    impl<S: Stream> SelectAll<S> {
        /// Constructs a new, empty `SelectAll`
        ///
        /// The returned `SelectAll` does not contain any streams and, in this
        /// state, `SelectAll::poll` will return `Ok(Async::Ready(None))`.
        pub fn new() -> SelectAll<S> {
            SelectAll { inner: FuturesUnordered::new() }
        }

        /// Push a stream into the set.
        ///
        /// This function submits the given stream to the set for managing. This
        /// function will not call `poll` on the submitted stream. The caller must
        /// ensure that `SelectAll::poll` is called in order to receive task
        /// notifications.
        pub fn push(&mut self, stream: S) {
            self.inner.push(stream.next());
        }
    }

    impl<S: Stream> Stream for SelectAll<S> {
        type Item = S::Item;
        type Error = S::Error;

        fn poll_next(&mut self, cx: &mut task::Context) -> Poll<Option<Self::Item>, Self::Error> {
            loop {
                match self.inner.poll_next(cx).map_err(|(err, _)| err)? {
                    Async::Pending => return Ok(Async::Pending),
                    Async::Ready(Some((Some(item), remaining))) => {
                        self.push(remaining);
                        return Ok(Async::Ready(Some(item)));
                    }
                    Async::Ready(None) => return Ok(Async::Ready(None)),
                    Async::Ready(Some((None, _))) => {}
                }
            }
        }
    }

    /// Convert a list of streams into a `Stream` of results from the streams.
    ///
    /// This essentially takes a list of streams (e.g. a vector, an iterator, etc.)
    /// and bundles them together into a single stream.
    /// The stream will yield items as they become available on the underlying
    /// streams internally, in the order they become available.
    ///
    /// Note that the returned set can also be used to dynamically push more
    /// futures into the set as they become available.
    pub fn select_all<I>(streams: I) -> SelectAll<I::Item>
        where I: IntoIterator,
              I::Item: Stream
    {
        let mut set = SelectAll::new();

        for stream in streams {
            set.push(stream);
        }

        return set
    }
}
