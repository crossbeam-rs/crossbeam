use std::cell::Cell;
use std::time::{Duration, Instant};

use {Receiver, Sender};
use actor::{self, HandleId};
use err::{TryRecvError, TrySendError};

thread_local! {
    static MACHINE: Cell<Machine> = Cell::new(Machine::new());
}

pub(crate) fn send<T>(tx: &Sender<T>, value: T) -> Result<(), T> {
    MACHINE.with(|m| {
        let mut t = m.get();

        let res = if let Some(state) = t.step(tx.id()) {
            state.send(tx, value)
        } else {
            Err(value)
        };
        if res.is_ok() {
            actor::current().reset();
            t = Machine::new();
        }

        m.set(t);
        res
    })
}

pub(crate) fn recv<T>(rx: &Receiver<T>) -> Result<T, ()> {
    MACHINE.with(|m| {
        let mut t = m.get();

        let res = if let Some(state) = t.step(rx.id()) {
            state.recv(rx)
        } else {
            Err(())
        };
        if res.is_ok() {
                actor::current().reset();
            t = Machine::new();
        }

        m.set(t);
        res
    })
}

#[inline]
pub fn disconnected() -> bool {
    MACHINE.with(|m| {
        if let Machine::Initialized { state, .. } = m.get() {
            if let State::Disconnected = state {
                actor::current().reset();
                m.set(Machine::new());
                return true;
            }
        }
        false
    })
}

#[inline]
pub fn blocked() -> bool {
    MACHINE.with(|m| {
        let mut t = m.get();

        if let Machine::Initialized { state, .. } = t {
            if state == State::Blocked {
                actor::current().reset();
                m.set(Machine::new());
                return true;
            }
        }

        if let Machine::Counting {
            ref mut seen_blocked,
            ..
        } = t
        {
            *seen_blocked = true;
        }

        m.set(t);
        false
    })
}

#[inline]
pub fn timeout(dur: Duration) -> bool {
    MACHINE.with(|m| {
        let mut t = m.get();

        if let Machine::Initialized { state, .. } = m.get() {
            if let State::Timeout = state {
                actor::current().reset();
                m.set(Machine::new());
                return true;
            }
        }

        if let Machine::Counting {
            ref mut deadline, ..
        } = t
        {
            *deadline = Some(Instant::now() + dur);
        }

        m.set(t);
        false
    })
}

#[derive(Clone, Copy)]
enum Machine {
    Counting {
        len: usize,
        first_id: HandleId,
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

struct Config {
    len: usize,
    start: usize,
    deadline: Option<Instant>,
}

impl Machine {
    #[inline]
    fn new() -> Self {
        Machine::Counting {
            len: 0,
            first_id: HandleId::sentinel(),
            deadline: None,
            seen_blocked: false,
        }
    }

    #[inline]
    fn step(&mut self, id: HandleId) -> Option<&mut State> {
        loop {
            match *self {
                Machine::Counting {
                    len,
                    first_id,
                    deadline,
                    seen_blocked,
                } => if first_id == id {
                    actor::current().reset();

                    *self = Machine::Initialized {
                        pos: 0,
                        state: if seen_blocked {
                            State::TryOnce { closed_count: 0 }
                        } else {
                            State::SpinTry { closed_count: 0 }
                        },
                        len,
                        start: gen_random(len),
                        deadline,
                    };
                } else {
                    *self = Machine::Counting {
                        len: len + 1,
                        first_id: if first_id == HandleId::sentinel() {
                            id
                        } else {
                            first_id
                        },
                        deadline,
                        seen_blocked,
                    };
                    return None;
                },
                Machine::Initialized {
                    pos,
                    mut state,
                    len,
                    start,
                    deadline,
                } => if pos >= 2 * len {
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
                },
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    TryOnce { closed_count: usize },
    SpinTry { closed_count: usize },
    Promise { closed_count: usize },
    Revoke,
    Fulfill { id: HandleId },
    Disconnected,
    Blocked,
    Timeout,
}

impl State {
    #[inline]
    fn transition(&mut self, len: usize, deadline: Option<Instant>) {
        match *self {
            State::TryOnce { closed_count } => if closed_count < len {
                *self = State::Blocked;
            } else {
                *self = State::Disconnected;
            },
            State::SpinTry { closed_count } => if closed_count < len {
                actor::current().reset();
                *self = State::Promise { closed_count: 0 };
            } else {
                *self = State::Disconnected;
            },
            State::Promise { closed_count } => {
                if closed_count < len {
                    actor::current().wait_until(deadline);
                } else {
                    actor::current().select(HandleId::sentinel());
                }
                *self = State::Revoke;
            }
            State::Revoke => {
                *self = State::Fulfill {
                    id: actor::current().selected(),
                };
            }
            State::Fulfill { .. } => {
                actor::current().reset();
                *self = State::SpinTry { closed_count: 0 };

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
                ref mut closed_count,
            } => {
                match tx.try_send(value) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(v)) => value = v,
                    Err(TrySendError::Disconnected(v)) => {
                        value = v;
                        *closed_count += 1;
                    }
                }
            }
            State::SpinTry {
                ref mut closed_count,
            } => {
                match tx.spin_try_send(value) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(v)) => value = v,
                    Err(TrySendError::Disconnected(v)) => {
                        value = v;
                        *closed_count += 1;
                    }
                }
            },
            State::Promise {
                ref mut closed_count,
            } => {
                tx.promise_send();

                if tx.is_disconnected() {
                    *closed_count += 1;
                } else if tx.can_send() {
                    actor::current().select(HandleId::sentinel());
                }
            }
            State::Revoke => tx.revoke_send(),
            State::Fulfill { id } => if tx.id() == id {
                match tx.fulfill_send(value) {
                    Ok(()) => return Ok(()),
                    Err(v) => value = v,
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
                ref mut closed_count,
            } => match rx.try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => *closed_count += 1,
            },
            State::SpinTry {
                ref mut closed_count,
            } => match rx.spin_try_recv() {
                Ok(v) => return Ok(v),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => *closed_count += 1,
            },
            State::Promise {
                ref mut closed_count,
            } => {
                rx.promise_recv();

                if rx.is_disconnected() {
                    *closed_count += 1;
                } else if rx.can_recv() {
                    actor::current().select(HandleId::sentinel());
                }
            }
            State::Revoke => rx.revoke_recv(),
            State::Fulfill { id } => if rx.id() == id {
                if let Ok(v) = rx.fulfill_recv() {
                    return Ok(v);
                }
            },
            State::Disconnected => {}
            State::Blocked => {}
            State::Timeout => {}
        }
        Err(())
    }
}

fn gen_random(ceil: usize) -> usize {
    use std::num::Wrapping;

    thread_local! {
        static RNG: Cell<Wrapping<u32>> = Cell::new(Wrapping(1));
    }

    RNG.with(|rng| {
        let mut x = rng.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rng.set(x);

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
