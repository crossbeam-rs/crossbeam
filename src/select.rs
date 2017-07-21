use std::marker::PhantomData;
use std::thread;
use std::time::{Duration, Instant};

use rand::{Rng, thread_rng};

use channel::Channel;
use channel::Receiver;
use err::TryRecvError;

// TODO: impl !Send and !Sync

pub struct Select {
    machine: Machine,
    _marker: PhantomData<*mut ()>,
}

#[derive(Clone, Copy)]
enum Machine {
    Counting {
        len: usize,
        id_first: usize,
        deadline: Option<Instant>,
    },
    Initialized { config: Config, state: State },
}

#[derive(Clone, Copy)]
struct Config {
    len: usize,
    start: usize,
    deadline: Option<Instant>,
}

impl Config {
    fn is_end(&self, index: usize) -> bool {
        index >= 2 * self.len
    }

    fn is_valid(&self, index: usize) -> bool {
        self.start <= index && index < self.start + self.len
    }
}

#[derive(Clone, Copy)]
enum State {
    TryRecv { pos: usize, all_disconnected: bool },
    Subscribe { pos: usize },
    IsReady { pos: usize, any_ready: bool },
    Unsubscribe { pos: usize, id_woken: usize },
    FinalTryRecv { pos: usize, id_woken: usize },
    TimedOut,
    Disconnected,
}

enum Poll<S, T> {
    Move(S),
    Skip(S),
    Success(T),
}

impl Select {
    #[inline]
    pub fn new() -> Self {
        Select {
            machine: Machine::Counting {
                len: 0,
                id_first: 0,
                deadline: None,
            },
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn with_timeout(dur: Duration) -> Self {
        Select {
            machine: Machine::Counting {
                len: 0,
                id_first: 0,
                deadline: Some(Instant::now() + dur),
            },
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn timed_out(&self) -> bool {
        if let Machine::Initialized { state, .. } = self.machine {
            if let State::TimedOut = state {
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn disconnected(&self) -> bool {
        if let Machine::Initialized { state, .. } = self.machine {
            if let State::Disconnected = state {
                return true;
            }
        }
        false
    }

    pub fn poll<T>(&mut self, rx: &Receiver<T>) -> Option<T> {
        let chan = rx.as_channel();

        loop {
            match poll_machine(self.machine, chan) {
                Poll::Move(m) => self.machine = m,
                Poll::Skip(m) => {
                    self.machine = m;
                    return None;
                }
                Poll::Success(t) => return Some(t),
            }
        }
    }
}

fn poll_machine<T>(machine: Machine, chan: &Channel<T>) -> Poll<Machine, T> {
    match machine {
        Machine::Counting {
            len,
            id_first,
            deadline,
        } => {
            if id_first == chan.id() {
                Poll::Move(Machine::Initialized {
                    config: Config {
                        len,
                        start: thread_rng().gen_range(0, len),
                        deadline,
                    },
                    state: State::TryRecv {
                        pos: 0,
                        all_disconnected: true,
                    },
                })
            } else {
                Poll::Skip(Machine::Counting {
                    len: len + 1,
                    id_first: if id_first == 0 { chan.id() } else { id_first },
                    deadline,
                })
            }
        }
        Machine::Initialized { config, state } => {
            match poll_state(state, config, chan) {
                Poll::Move(state) => Poll::Move(Machine::Initialized { config, state }),
                Poll::Skip(state) => Poll::Skip(Machine::Initialized { config, state }),
                Poll::Success(t) => Poll::Success(t),
            }
        }
    }
}

fn poll_state<T>(state: State, config: Config, chan: &Channel<T>) -> Poll<State, T> {
    match state {
        State::TryRecv {
            pos,
            all_disconnected,
        } => {
            if config.is_end(pos) {
                if all_disconnected {
                    Poll::Skip(State::TimedOut)
                } else {
                    Poll::Move(State::Subscribe { pos: 0 })
                }
            } else {
                let disconnected = if config.is_valid(pos) {
                    match chan.try_recv() {
                        Ok(t) => return Poll::Success(t),
                        Err(TryRecvError::Disconnected) => true,
                        Err(TryRecvError::Empty) => false,
                    }
                } else {
                    false
                };
                Poll::Skip(State::TryRecv {
                    pos: pos + 1,
                    all_disconnected: all_disconnected && disconnected,
                })
            }
        }
        State::Subscribe { pos } => {
            if config.is_end(pos) {
                Poll::Move(State::IsReady {
                    pos: 0,
                    any_ready: false,
                })
            } else {
                if config.is_valid(pos) {
                    chan.monitor().watch_start();
                }
                Poll::Skip(State::Subscribe { pos: pos + 1 })
            }
        }
        State::IsReady { pos, any_ready } => {
            if config.is_end(pos) {
                if !any_ready {
                    let now = Instant::now();
                    if let Some(end) = config.deadline {
                        if now < end {
                            thread::park_timeout(end - now);
                        }
                    } else {
                        thread::park();
                    }
                }
                Poll::Move(State::Unsubscribe {
                    pos: 0,
                    id_woken: 0,
                })
            } else {
                let ready = if config.is_valid(pos) {
                    chan.is_ready()
                } else {
                    false
                };
                Poll::Skip(State::IsReady {
                    pos: pos + 1,
                    any_ready: any_ready || ready,
                })
            }
        }
        State::Unsubscribe { pos, id_woken } => {
            if config.is_end(pos) {
                Poll::Move(State::FinalTryRecv { pos: 0, id_woken })
            } else {
                if config.is_valid(pos) && id_woken != 0 {
                    chan.monitor().watch_abort();
                }
                Poll::Skip(State::Unsubscribe {
                    pos: pos + 1,
                    id_woken: if config.is_valid(pos) && id_woken == 0 &&
                        !chan.monitor().watch_stop()
                    {
                        chan.id()
                    } else {
                        id_woken
                    },
                })
            }
        }
        State::FinalTryRecv { pos, id_woken } => {
            if config.is_end(pos) {
                if let Some(end) = config.deadline {
                    if Instant::now() >= end {
                        return Poll::Skip(State::TimedOut);
                    }
                }
                Poll::Move(State::TryRecv {
                    pos: 0,
                    all_disconnected: true,
                })
            } else {
                if config.is_valid(pos) && chan.id() == id_woken {
                    if let Ok(t) = chan.try_recv() {
                        return Poll::Success(t);
                    }
                }
                Poll::Skip(State::FinalTryRecv {
                    pos: pos + 1,
                    id_woken,
                })
            }
        }
        State::TimedOut => Poll::Skip(State::TimedOut),
        State::Disconnected => Poll::Skip(State::Disconnected),
    }
}

#[cfg(test)]
mod tests {
    use crossbeam;

    use super::*;
    use bounded;
    use unbounded;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn select() {
        let (tx1, rx1) = bounded::<i32>(0);
        let (tx2, rx2) = bounded::<i32>(0);

        crossbeam::scope(|s| {
            s.spawn(|| {
                thread::sleep(ms(150));
                tx1.send_timeout(1, ms(200));
            });
            s.spawn(|| {
                thread::sleep(ms(150));
                tx2.send_timeout(2, ms(200));
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                // let mut s = Select::new();
                let mut s = Select::with_timeout(ms(100));
                loop {
                    if let Some(x) = s.poll(&rx1) {
                        println!("{}", x);
                        break;
                    }
                    if let Some(x) = s.poll(&rx2) {
                        println!("{}", x);
                        break;
                    }
                    if s.disconnected() {
                        println!("DISCONNECTED!");
                        break;
                    }
                    if s.timed_out() {
                        println!("TIMEOUT!");
                        break;
                    }
                }
            });
        });
    }
}
