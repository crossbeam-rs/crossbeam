use std::cell::RefCell;
use std::time::Instant;

use {Receiver, Sender};
use err::{TryRecvError, TrySendError};
use select::handle;
use select::CaseId;
use util;

thread_local! {
    /// The thread-local selection state machine.
    pub static MACHINE: RefCell<Machine> = RefCell::new(Machine::new());
}

pub enum Machine {
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
    pub fn new() -> Self {
        Machine::Counting {
            len: 0,
            first_id: CaseId::none(),
            deadline: None,
            seen_blocked: false,
        }
    }

    #[inline]
    pub fn step(&mut self, case_id: CaseId) -> Option<&mut State> {
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
                            start: util::small_random(len),
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
pub enum State {
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
    pub fn transition(&mut self, len: usize, deadline: Option<Instant>) {
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
                    handle::current_reset();
                    *self = State::Promise { disconn_count: 0 };
                } else {
                    *self = State::Disconnected;
                }
            },
            State::Promise { disconn_count } => {
                if disconn_count < len {
                    handle::current_wait_until(deadline);
                } else {
                    handle::current_select(CaseId::abort());
                }
                *self = State::Revoke {
                    case_id: handle::current_selected(),
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

    pub fn send<T>(&mut self, tx: &Sender<T>, mut value: T) -> Result<(), T> {
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
                    handle::current_select(CaseId::abort());
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

    pub fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
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
                    handle::current_select(CaseId::abort());
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
