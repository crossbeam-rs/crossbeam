use std::time::Instant;

use {Receiver, Sender};
use err::{TryRecvError, TrySendError};
use select::handle;
use select::CaseId;
use utils;

// TODO(stjepang): Explain operation priorities and write tests to verify them:
// 1. send/recv
// 2. all_closed
// 3. any_closed
// 4. would_block
// 5. timed_out

#[derive(Clone, Copy, Eq, PartialEq)]
enum State {
    Count,
    Try { closed_count: usize },
    Promise { closed_count: usize },
    Revoke { case_id: CaseId },
    Fulfill { case_id: CaseId },
    Closed,
    WouldBlock,
    TimedOut,
    Dead,
}

// impl State {
//     #[inline]
//     fn is_final(&self) -> bool {
//         match *self {
//             State::Closed | State::WouldBlock | State::TimedOut => true,
//             _ => false,
//         }
//     }
// }

pub struct Machine {
    state: State,
    index: usize,
    start_index: usize,
    first_id: CaseId,
    deadline: Option<Instant>,

    len: usize,
    send_case_count: usize,
    recv_case_count: usize,
    has_closed_case: bool,
    has_would_block_case: bool,
    has_timed_out_case: bool,
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
            first_id: CaseId::none(),
            deadline,

            len: 0,
            send_case_count: 0,
            recv_case_count: 0,
            has_closed_case: false,
            has_would_block_case: false,
            has_timed_out_case: false,
        }
    }

    #[inline(always)]
    pub fn send<T>(&mut self, tx: &Sender<T>, mut msg: T) -> Result<(), T> {
        if !self.step(tx.case_id()) {
            return Err(msg);
        }

        match self.state {
            State::Try {
                ref mut closed_count,
            } => {
                match tx.try_send(msg) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Full(m)) => msg = m,
                    Err(TrySendError::Closed(m)) => {
                        msg = m;
                        *closed_count += 1;
                    }
                }
            },
            State::Promise {
                ref mut closed_count,
            } => {
                tx.promise_send();

                if tx.is_closed() {
                    *closed_count += 1;
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
            State::Count | State::Closed | State::WouldBlock | State::TimedOut => {}
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
                ref mut closed_count,
            } => {
                match rx.try_recv() {
                    Ok(m) => return Ok(m),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Closed) => *closed_count += 1,
                }
            },
            State::Promise {
                ref mut closed_count,
            } => {
                rx.promise_recv();

                let is_closed = rx.is_closed();
                let can_recv = rx.can_recv();

                if is_closed && !can_recv {
                    *closed_count += 1;
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
            State::Count | State::Closed | State::WouldBlock | State::TimedOut => {}
            State::Dead => panic!("cannot use the same `Select` for multiple selections")
        }
        Err(())
    }

    #[inline]
    pub fn closed(&mut self) -> bool {
        if !self.step(CaseId::closed()) {
            return false;
        }
        self.state == State::Closed
    }

    #[inline]
    pub fn would_block(&mut self) -> bool {
        if !self.step(CaseId::would_block()) {
            return false;
        }
        self.state == State::WouldBlock
    }

    #[inline]
    pub fn timed_out(&mut self) -> bool {
        if !self.step(CaseId::timed_out()) {
            return false;
        }
        self.state == State::TimedOut
    }

    #[inline(always)]
    pub fn step(&mut self, case_id: CaseId) -> bool {
        assert!(
            self.state != State::Dead,
            "cannot use the same `Select` for multiple selections"
        );

        if self.state == State::Count {
            if self.first_id != case_id {
                if self.len == 0 {
                    self.first_id = case_id;
                }

                self.len += 1;
                self.index += 1;

                self.send_case_count += case_id.is_send() as usize;
                self.recv_case_count += case_id.is_recv() as usize;
                self.has_closed_case |= case_id == CaseId::closed();
                self.has_would_block_case |= case_id == CaseId::would_block();
                self.has_timed_out_case |= case_id == CaseId::timed_out();

                return false;
            }

            self.state = State::Try {
                closed_count: 0,
            };
            self.index = 0;
            self.start_index = utils::small_random(self.len);
        }

        if self.index >= 2 * self.len {
            self.transition();
            self.index = 0;
        }

        let i = self.index;
        self.index += 1;
        self.start_index <= i && i < self.start_index + self.len
    }

    #[inline(always)]
    fn transition(&mut self) {
        match self.state {
            State::Try { closed_count } => {
                let all_closed =
                    self.send_case_count + self.recv_case_count == closed_count;

                if self.has_closed_case && all_closed {
                    self.state = State::Closed;
                } else if self.has_would_block_case {
                    self.state = State::WouldBlock;
                } else {
                    handle::current_reset();
                    self.state = State::Promise { closed_count: 0 };
                }
            }
            State::Promise { closed_count } => {
                let all_closed =
                    self.send_case_count + self.recv_case_count == closed_count;

                if self.has_closed_case && all_closed {
                    handle::current_try_select(CaseId::abort());
                } else {
                    handle::current_wait_until(self.deadline);
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
                    closed_count: 0,
                };

                if let Some(end) = self.deadline {
                    if Instant::now() >= end {
                        self.state = State::TimedOut;
                    }
                }
            }
            State::Closed => {}
            State::WouldBlock => {}
            State::TimedOut => {}
            State::Count => {}
            State::Dead => panic!("cannot use the same `Select` for multiple selections")
        }
    }
}
