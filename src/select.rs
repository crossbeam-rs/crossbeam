use std::cell::{Cell, RefCell};
use std::num::Wrapping;
use std::time::{Duration, Instant};

use {Receiver, Sender};
use actor;
use err::{TryRecvError, TrySendError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CaseId {
    pub id: usize,
}

impl CaseId {
    #[inline]
    pub fn new(id: usize) -> Self {
        CaseId { id }
    }

    #[inline]
    pub fn none() -> Self {
        CaseId { id: 0 }
    }

    #[inline]
    pub fn abort() -> Self {
        CaseId { id: 1 }
    }
}

thread_local! {
    /// The thread-local selection state machine.
    static MACHINE: RefCell<Machine> = RefCell::new(Machine::new());
}

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

enum Machine {
    Counting {
        len: usize,
        first_id: CaseId,
        deadline: Option<Instant>,
        seen_blocked: bool,
    },
    Initialized {
        pos: usize,
        state: State,
        len: usize,
        start: usize,
        deadline: Option<Instant>,
    },
}

impl Machine {
    #[inline]
    fn new() -> Self {
        Machine::Counting {
            len: 0,
            first_id: CaseId::none(),
            deadline: None,
            seen_blocked: false,
        }
    }

    #[inline]
    fn step(&mut self, case_id: CaseId) -> Option<&mut State> {
        // Non-lexical lifetimes will make all this much easier...
        loop {
            match *self {
                Machine::Counting {
                    len,
                    first_id,
                    deadline,
                    seen_blocked,
                } => {
                    if first_id == case_id {
                        *self = Machine::Initialized {
                            pos: 0,
                            state: {
                                if seen_blocked {
                                    State::TryOnce { disconn_count: 0 }
                                } else {
                                    State::SpinTry { disconn_count: 0 }
                                }
                            },
                            len,
                            start: gen_random(len),
                            deadline,
                        };
                    } else {
                        *self = Machine::Counting {
                            len: len + 1,
                            first_id: {
                                if first_id == CaseId::none() {
                                    case_id
                                } else {
                                    first_id
                                }
                            },
                            deadline,
                            seen_blocked,
                        };
                        return None;
                    }
                },
                Machine::Initialized {
                    pos,
                    mut state,
                    len,
                    start,
                    deadline,
                } => {
                    if pos >= 2 * len {
                        state.transition(len, deadline);
                        *self = Machine::Initialized {
                            pos: 0,
                            state,
                            len,
                            start,
                            deadline,
                        };
                    } else {
                        *self = Machine::Initialized {
                            pos: pos + 1,
                            state,
                            len,
                            start,
                            deadline,
                        };
                        if let Machine::Initialized { ref mut state, .. } = *self {
                            if start <= pos && pos < start + len {
                                return Some(state);
                            } else {
                                return None;
                            }
                        }
                    }
                },
            }
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    TryOnce { disconn_count: usize },
    SpinTry { disconn_count: usize },
    Promise { disconn_count: usize },
    Revoke { case_id: CaseId },
    Fulfill { case_id: CaseId },
    Disconnected,
    Blocked,
    Timeout,
}

impl State {
    #[inline]
    fn transition(&mut self, len: usize, deadline: Option<Instant>) {
        match *self {
            State::TryOnce { disconn_count } => {
                if disconn_count < len {
                    *self = State::Blocked;
                } else {
                    *self = State::Disconnected;
                }
            },
            State::SpinTry { disconn_count } => {
                if disconn_count < len {
                    actor::current_reset();
                    *self = State::Promise { disconn_count: 0 };
                } else {
                    *self = State::Disconnected;
                }
            },
            State::Promise { disconn_count } => {
                if disconn_count < len {
                    actor::current_wait_until(deadline);
                } else {
                    actor::current_select(CaseId::abort());
                }
                *self = State::Revoke {
                    case_id: actor::current_selected(),
                };
            }
            State::Revoke { case_id } => {
                *self = State::Fulfill { case_id };
            }
            State::Fulfill { .. } => {
                *self = State::SpinTry { disconn_count: 0 };

                if let Some(end) = deadline {
                    if Instant::now() >= end {
                        *self = State::Timeout;
                    }
                }
            }
            State::Disconnected => {}
            State::Blocked => {}
            State::Timeout => {}
        }
    }

    fn send<T>(&mut self, tx: &Sender<T>, mut value: T) -> Result<(), T> {
        match *self {
            State::TryOnce {
                ref mut disconn_count,
            } => {
                match tx.try_send(value) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(v)) => value = v,
                    Err(TrySendError::Disconnected(v)) => {
                        value = v;
                        *disconn_count += 1;
                    }
                }
            },
            State::SpinTry {
                ref mut disconn_count,
            } => {
                match tx.spin_try_send(value) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(v)) => value = v,
                    Err(TrySendError::Disconnected(v)) => {
                        value = v;
                        *disconn_count += 1;
                    }
                }
            },
            State::Promise {
                ref mut disconn_count,
            } => {
                tx.promise_send();

                if tx.is_disconnected() {
                    *disconn_count += 1;
                } else if tx.can_send() {
                    actor::current_select(CaseId::abort());
                }
            }
            State::Revoke { case_id } => {
                if tx.case_id() != case_id {
                    tx.revoke_send();
                }
            },
            State::Fulfill { case_id } => {
                if tx.case_id() == case_id {
                    match tx.fulfill_send(value) {
                        Ok(()) => return Ok(()),
                        Err(v) => value = v,
                    }
                }
            },
            State::Disconnected => {}
            State::Blocked => {}
            State::Timeout => {}
        }
        Err(value)
    }

    fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        match *self {
            State::TryOnce {
                ref mut disconn_count,
            } => {
                match rx.try_recv() {
                    Ok(v) => return Ok(v),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => *disconn_count += 1,
                }
            },
            State::SpinTry {
                ref mut disconn_count,
            } => {
                match rx.spin_try_recv() {
                    Ok(v) => return Ok(v),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => *disconn_count += 1,
                }
            },
            State::Promise {
                ref mut disconn_count,
            } => {
                rx.promise_recv();

                let is_disconn = rx.is_disconnected();
                let can_recv = rx.can_recv();

                if is_disconn && !can_recv {
                    *disconn_count += 1;
                } else if can_recv {
                    actor::current_select(CaseId::abort());
                }
            }
            State::Revoke { case_id } => {
                if rx.case_id() != case_id {
                    rx.revoke_recv();
                }
            },
            State::Fulfill { case_id } => {
                if rx.case_id() == case_id {
                    if let Ok(v) = rx.fulfill_recv() {
                        return Ok(v);
                    }
                }
            },
            State::Disconnected => {}
            State::Blocked => {}
            State::Timeout => {}
        }
        Err(())
    }
}

/// Returns a random number in range `0..ceil`.
fn gen_random(ceil: usize) -> usize {
    thread_local! {
        static RNG: Cell<Wrapping<u32>> = Cell::new(Wrapping(1));
    }

    RNG.with(|rng| {
        // This is the 32-bit variant of Xorshift.
        let mut x = rng.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rng.set(x);

        // A modulo operation by a constant gets compiled to simple multiplication. The general
        // modulo instruction is in comparison much more expensive, so here we match on the most
        // common moduli in order to avoid it.
        let x = x.0 as usize;
        match ceil {
            1 => x % 1,
            2 => x % 2,
            3 => x % 3,
            4 => x % 4,
            5 => x % 5,
            6 => x % 6,
            7 => x % 7,
            n => x % n,
        }
    })
}
