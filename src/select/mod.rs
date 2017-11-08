use std::time::{Duration, Instant};

use {Receiver, Sender};
use self::machine::{MACHINE, Machine, State};

pub(crate) use self::case_id::CaseId;

mod case_id;
mod machine;

pub(crate) mod handle; // TODO: make private

// TODO: explain that selection cannot have repeated cases on the same side of a channel.

#[inline(never)]
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

#[inline(never)]
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

pub fn would_block() -> bool {
    MACHINE.with(|m| {
        let mut m = m.borrow_mut();

        if let Machine::Initialized {
            state: State::WouldBlock,
            ..
        } = *m
        {
            *m = Machine::new();
            return true;
        }

        if let Machine::Counting {
            ref mut dont_block,
            ..
        } = *m
        {
            *dont_block = true;
        }

        false
    })
}

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
