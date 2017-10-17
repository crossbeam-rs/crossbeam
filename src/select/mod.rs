use std::time::{Duration, Instant};

use {Receiver, Sender};
use self::machine::{MACHINE, Machine, State};

pub(crate) use self::case_id::CaseId;

mod case_id;
mod machine;

pub(crate) mod handle; // TODO: make private

pub(crate) fn send<T>(tx: &Sender<T>, value: T) -> Result<(), T> {
    MACHINE.with(|m| {
        let mut t = m.borrow_mut();

        let res = if let Some(state) = t.step(tx.case_id()) {
            state.send(tx, value)
        } else {
            Err(value)
        };

        if res.is_ok() {
            *t = Machine::new();
        }
        res
    })
}

pub(crate) fn recv<T>(rx: &Receiver<T>) -> Result<T, ()> {
    MACHINE.with(|m| {
        let mut m = m.borrow_mut();

        let res = if let Some(state) = m.step(rx.case_id()) {
            state.recv(rx)
        } else {
            Err(())
        };

        if res.is_ok() {
            *m = Machine::new();
        }
        res
    })
}

#[inline]
pub fn disconnected() -> bool {
    MACHINE.with(|m| {
        let mut m = m.borrow_mut();

        if let Machine::Initialized {
            state: State::Disconnected,
            ..
        } = *m {
            *m = Machine::new();
            true
        } else {
            false
        }
    })
}

#[inline]
pub fn blocked() -> bool {
    MACHINE.with(|m| {
        let mut m = m.borrow_mut();

        if let Machine::Initialized {
            state: State::Blocked,
            ..
        } = *m
        {
            *m = Machine::new();
            return true;
        }

        if let Machine::Counting {
            ref mut seen_blocked,
            ..
        } = *m
        {
            *seen_blocked = true;
        }

        false
    })
}

#[inline]
pub fn timeout(dur: Duration) -> bool {
    MACHINE.with(|m| {
        let mut m = m.borrow_mut();

        if let Machine::Initialized {
            state: State::Timeout,
            ..
        } = *m {
            *m = Machine::new();
            return true;
        }

        if let Machine::Counting {
            ref mut deadline, ..
        } = *m
        {
            *deadline = Some(Instant::now() + dur);
        }

        false
    })
}
