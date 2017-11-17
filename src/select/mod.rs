use std::time::{Duration, Instant};

use {Receiver, Sender};
use err::SelectRecvError;
use self::machine::{Machine, State};

pub(crate) use self::case_id::CaseId;

mod case_id;
mod machine;

pub(crate) mod handle;

// TODO: explain that selection cannot have repeated cases on the same side of a channel.

pub struct Select {
    machine: Machine,
}

impl Select {
    #[inline]
    pub fn new() -> Select {
        Select {
            machine: Machine::new(),
        }
    }

    #[inline]
    pub fn with_timeout(dur: Duration) -> Select {
        Select {
            machine: Machine::with_deadline(Some(Instant::now() + dur)),
        }
    }

    pub fn send<T>(&mut self, tx: &Sender<T>, value: T) -> Result<(), T> {
        if let Some(state) = self.machine.step(tx.case_id()) {
            state.send(tx, value)
        } else {
            Err(value)
        }
    }

    pub fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, SelectRecvError> {
        if let Some(state) = self.machine.step(rx.case_id()) {
            state.recv(rx).map_err(|_| SelectRecvError)
        } else {
            Err(SelectRecvError)
        }
    }

    #[inline]
    pub fn disconnected(&mut self) -> bool {
        if let Machine::Initialized {
            state: State::Disconnected,
            ..
        } = self.machine {
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn would_block(&mut self) -> bool {
        if let Machine::Initialized {
            state: State::WouldBlock,
            ..
        } = self.machine
        {
            return true;
        }

        if let Machine::Counting {
            ref mut dont_block,
            ..
        } = self.machine
        {
            *dont_block = true;
        }

        false
    }

    #[inline]
    pub fn timed_out(&self) -> bool {
        if let Machine::Initialized {
            state: State::Timeout,
            ..
        } = self.machine {
            true
        } else {
            false
        }
    }
}
