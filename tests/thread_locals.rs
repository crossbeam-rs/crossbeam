//! Tests that make sure accessing thread-locals while exiting the thread doesn't cause panics.

#![deny(unsafe_code)]

extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

use std::thread;
use std::time::Duration;

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

#[test]
fn use_while_exiting() {
    struct Foo;

    impl Drop for Foo {
        fn drop(&mut self) {
            // A blocking operation after the thread-locals have been dropped. This will attempt to
            // use the thread-locals and must not panic.
            let (_s, r) = channel::unbounded::<()>();
            select! {
                recv(r) => {}
                recv(channel::after(ms(100))) => {}
            }
        }
    }

    thread_local! {
        static FOO: Foo = Foo;
    }

    let (s, r) = channel::unbounded::<()>();

    crossbeam::scope(|scope| {
        scope.spawn(|| {
            // First initialize `FOO`, then the thread-locals related to crossbeam-channel and
            // crossbeam-epoch.
            FOO.with(|_| ());
            r.recv().unwrap();
            // At thread exit, the crossbeam-related thread-locals get dropped first and `FOO` is
            // dropped last.
        });

        scope.spawn(|| {
            thread::sleep(ms(100));
            s.send(()).unwrap();
        });
    });
}
