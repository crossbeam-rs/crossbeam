use std::mem;
use std::time::Instant;

use {Receiver, Sender};
use err::{TryRecvError, TrySendError};
use select::handle;
use select::CaseId;
use util;

#[derive(Clone, Copy, Eq, PartialEq)]
enum State {
    Count,
    Try { disconnected_count: usize },
    Promise { disconnected_count: usize },
    Revoke { case_id: CaseId },
    Fulfill { case_id: CaseId },
    Disconnected,
    WouldBlock,
    TimedOut,
    Dead,
}

impl State {
    #[inline]
    fn is_final(&self) -> bool {
        match *self {
            State::Disconnected | State::WouldBlock | State::TimedOut => true,
            _ => false,
        }
    }
}

pub struct Machine {
    state: State,
    index: usize,
    start_index: usize,
    len: usize,
    first_id: CaseId,
    deadline: Option<Instant>,
    seen_disconnected_case: bool,
    seen_would_block_case: bool,
}

impl Machine {
    #[inline]
    pub fn new() -> Self {
        Self::with_deadline(None)
    }

    #[inline]
    pub fn with_deadline(deadline: Option<Instant>) -> Self {
        Machine {
            state: State::Count,
            index: 0,
            start_index: 0,
            len: 0,
            first_id: CaseId::none(),
            deadline,
            seen_disconnected_case: false,
            seen_would_block_case: false,
        }
    }

    #[inline(always)]
    pub fn send<T>(&mut self, tx: &Sender<T>, mut msg: T) -> Result<(), T> {
        if !self.step(tx.case_id()) {
            return Err(msg);
        }

        match self.state {
            State::Try {
                ref mut disconnected_count,
            } => {
                match tx.try_send(msg) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(m)) => msg = m,
                    Err(TrySendError::Disconnected(m)) => {
                        msg = m;
                        *disconnected_count += 1;
                    }
                }
            },
            State::Promise {
                ref mut disconnected_count,
            } => {
                tx.promise_send();

                if tx.is_disconnected() {
                    *disconnected_count += 1;
                } else if tx.can_send() {
                    handle::current_try_select(CaseId::abort());
                }
            }
            State::Revoke { case_id } => {
                if tx.case_id() != case_id {
                    tx.revoke_send();
                }
            },
            State::Fulfill { case_id } => {
                if tx.case_id() == case_id {
                    match tx.fulfill_send(msg) {
                        Ok(()) => return Ok(()),
                        Err(m) => msg = m,
                    }
                }
            },
            State::Disconnected => {}
            State::WouldBlock => {}
            State::TimedOut => {}
            State::Count => {}
            State::Dead => panic!("cannot use the same `Select` for multiple selections")
        }
        Err(msg)
    }

    #[inline(always)]
    pub fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        if !self.step(rx.case_id()) {
            return Err(());
        }

        match self.state {
            State::Try {
                ref mut disconnected_count,
            } => {
                match rx.try_recv() {
                    Ok(m) => return Ok(m),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => *disconnected_count += 1,
                }
            },
            State::Promise {
                ref mut disconnected_count,
            } => {
                rx.promise_recv();

                let is_disconn = rx.is_disconnected();
                let can_recv = rx.can_recv();

                if is_disconn && !can_recv {
                    *disconnected_count += 1;
                } else if can_recv {
                    handle::current_try_select(CaseId::abort());
                }
            }
            State::Revoke { case_id } => {
                if rx.case_id() != case_id {
                    rx.revoke_recv();
                }
            },
            State::Fulfill { case_id } => {
                if rx.case_id() == case_id {
                    if let Ok(m) = rx.fulfill_recv() {
                        return Ok(m);
                    }
                }
            },
            State::Disconnected => {}
            State::WouldBlock => {}
            State::TimedOut => {}
            State::Count => {}
            State::Dead => panic!("cannot use the same `Select` for multiple selections")
        }
        Err(())
    }

    #[inline]
    pub fn disconnected(&mut self) -> bool {
        match self.state {
            State::Count => {
                assert!(
                    !mem::replace(&mut self.seen_disconnected_case, true),
                    "there are multiple `disconnected` cases"
                );
                false
            }
            State::Disconnected => true,
            State::Dead => panic!("cannot use the same `Select` for multiple selections"),
            _ => false,
        }
    }

    #[inline]
    pub fn would_block(&mut self) -> bool {
        match self.state {
            State::Count => {
                assert!(
                    !mem::replace(&mut self.seen_would_block_case, true),
                    "there are multiple `would_block` cases"
                );
                false
            }
            State::WouldBlock => true,
            State::Dead => panic!("cannot use the same `Select` for multiple selections"),
            _ => false,
        }
    }

    #[inline]
    pub fn timed_out(&self) -> bool {
        match self.state {
            State::TimedOut => true,
            State::Dead => panic!("cannot use the same `Select` for multiple selections"),
            _ => false,
        }
    }

    #[inline(always)]
    pub fn step(&mut self, case_id: CaseId) -> bool {
        loop {
            if self.state == State::Count {
                if self.first_id == case_id {
                    self.state = State::Try {
                        disconnected_count: 0,
                    };
                    self.index = 0;
                    self.start_index = util::small_random(self.len);
                } else {
                    if self.len == 0 {
                        self.first_id = case_id;
                    }
                    self.len += 1;
                    self.index += 1;
                }

                return false;
            } else {
                if self.index >= 2 * self.len {
                    self.transition();
                    self.index = 0;

                    assert!(
                        self.state != State::Dead,
                        "cannot use the same `Select` for multiple selections"
                    );
                } else {
                    let index = self.index;
                    self.index += 1;

                    if self.start_index <= index && index < self.start_index + self.len {
                        return true;
                    } else {
                        return false;
                    }
                }
            }
        }
    }

    #[inline(always)]
    fn transition(&mut self) {
        match self.state {
            State::Try { disconnected_count } => {
                if disconnected_count == self.len {
                    self.state = State::Disconnected;
                } else if self.seen_would_block_case {
                    self.state = State::WouldBlock;
                } else {
                    handle::current_reset();
                    self.state = State::Promise { disconnected_count: 0 };
                }
            }
            State::Promise { disconnected_count } => {
                if disconnected_count < self.len {
                    handle::current_wait_until(self.deadline);
                } else {
                    handle::current_try_select(CaseId::abort());
                }
                self.state = State::Revoke {
                    case_id: handle::current_selected(),
                };
            }
            State::Revoke { case_id } => {
                self.state = State::Fulfill { case_id };
            }
            State::Fulfill { .. } => {
                self.state = State::Try {
                    disconnected_count: 0,
                };

                if let Some(end) = self.deadline {
                    if Instant::now() >= end {
                        self.state = State::TimedOut;
                    }
                }
            }
            State::Disconnected => {}
            State::WouldBlock => {}
            State::TimedOut => {}
            State::Count => {}
            State::Dead => panic!("cannot use the same `Select` for multiple selections")
        }
    }
}
