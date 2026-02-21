use std::thread::{sleep, spawn};
use std::time::Duration;

use crossbeam_utils::sync::{Parker, UnparkReason, UnparkResult};
use crossbeam_utils::thread;

#[test]
fn park_timeout_unpark_before() {
    let p = Parker::new();
    for _ in 0..10 {
        p.unparker().unpark();
        assert_eq!(
            p.park_timeout(Duration::from_millis(u32::MAX as u64)),
            UnparkReason::Unparked,
        );
    }
}

#[test]
fn park_timeout_unpark_not_called() {
    let p = Parker::new();
    for _ in 0..10 {
        assert_eq!(
            p.park_timeout(Duration::from_millis(10)),
            UnparkReason::Timeout,
        );
    }
}

#[test]
fn park_timeout_unpark_called_other_thread() {
    for _ in 0..10 {
        let p = Parker::new();
        let u = p.unparker().clone();

        thread::scope(|scope| {
            scope.spawn(move |_| {
                sleep(Duration::from_millis(50));
                u.unpark();
            });

            assert_eq!(
                p.park_timeout(Duration::from_millis(u32::MAX as u64)),
                UnparkReason::Unparked,
            );
        })
        .unwrap();
    }
}

#[test]
fn unpark_with_result_called_before_park() {
    let p = Parker::new();
    let u = p.unparker().clone();

    let result = u.unpark_with_result();
    assert_eq!(result, UnparkResult::NotParked);

    p.park(); // consume the token and immediately return
}

#[test]
fn unpark_with_result_called_after_park() {
    let p = Parker::new();
    let u = p.unparker().clone();

    let t = spawn(move || {
        sleep(Duration::from_millis(500));
        let result = u.unpark_with_result();
        assert_eq!(result, UnparkResult::Notified);
    });

    p.park(); // consume the token and immediately return
    t.join().unwrap();
}
