use std::marker::PhantomData;
use std::thread;
use std::time::{Duration, Instant};

use rand::{Rng, thread_rng};

use Receiver;
use err::TryRecvError;
use impls::Channel;

pub struct Select {
    machine: Machine,
    _marker: PhantomData<*mut ()>,
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
        match self.machine.poll(rx) {
            Ok(t) => Some(t),
            Err(m) => {
                self.machine = m;
                None
            }
        }
    }
}

fn id<T>(rx: &Receiver<T>) -> usize {
    rx.as_channel() as *const Channel<T> as *const u8 as usize
}

#[derive(Clone, Copy)]
enum Machine {
    Counting {
        len: usize,
        id_first: usize,
        deadline: Option<Instant>,
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
    fn poll<T>(mut self, rx: &Receiver<T>) -> Result<T, Machine> {
        loop {
            self = match self {
                Machine::Counting {
                    len,
                    id_first,
                    deadline,
                } => {
                    if id_first == id(rx) {
                        Machine::Initialized {
                            pos: 0,
                            state: State::TryRecv {
                                all_disconnected: true,
                            },
                            len,
                            start: thread_rng().gen_range(0, len),
                            deadline,
                        }
                    } else {
                        return Err(Machine::Counting {
                            len: len + 1,
                            id_first: if id_first == 0 { id(rx) } else { id_first },
                            deadline,
                        });
                    }
                }
                Machine::Initialized {
                    pos,
                    state,
                    len,
                    start,
                    deadline,
                } => {
                    if pos >= 2 * len {
                        Machine::Initialized {
                            pos: 0,
                            state: state.transition(deadline),
                            len,
                            start,
                            deadline,
                        }
                    } else if start <= pos && pos < start + len {
                        match state.poll(rx) {
                            Ok(t) => return Ok(t),
                            Err(s) => {
                                return Err(Machine::Initialized {
                                    pos: pos + 1,
                                    state: s,
                                    len,
                                    start,
                                    deadline,
                                })
                            }
                        }
                    } else {
                        return Err(Machine::Initialized {
                            pos: pos + 1,
                            state,
                            len,
                            start,
                            deadline,
                        });
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    TryRecv { all_disconnected: bool },
    Subscribe,
    IsReady { any_ready: bool },
    Unsubscribe { id_woken: usize },
    FinalTryRecv { id_woken: usize },
    TimedOut,
    Disconnected,
}

impl State {
    fn transition(self, deadline: Option<Instant>) -> State {
        match self {
            State::TryRecv { all_disconnected } => {
                if all_disconnected {
                    State::TimedOut
                } else {
                    State::Subscribe
                }
            }
            State::Subscribe => State::IsReady { any_ready: false },
            State::IsReady { any_ready } => {
                if !any_ready {
                    let now = Instant::now();
                    if let Some(end) = deadline {
                        if now < end {
                            thread::park_timeout(end - now);
                        }
                    } else {
                        thread::park();
                    }
                }
                State::Unsubscribe { id_woken: 0 }
            }
            State::Unsubscribe { id_woken } => State::FinalTryRecv { id_woken },
            State::FinalTryRecv { id_woken } => {
                if let Some(end) = deadline {
                    if Instant::now() >= end {
                        return State::TimedOut;
                    }
                }
                State::TryRecv {
                    all_disconnected: true,
                }
            }
            State::TimedOut => State::TimedOut,
            State::Disconnected => State::Disconnected,
        }
    }

    fn poll<T>(self, rx: &Receiver<T>) -> Result<T, State> {
        match self {
            State::TryRecv { all_disconnected } => {
                let disconnected = match rx.try_recv() {
                    Ok(t) => return Ok(t),
                    Err(TryRecvError::Disconnected) => true,
                    Err(TryRecvError::Empty) => false,
                };
                Err(State::TryRecv {
                    all_disconnected: all_disconnected && disconnected,
                })
            }
            State::Subscribe => {
                rx.as_channel().monitor().watch_start();
                Err(State::Subscribe)
            }
            State::IsReady { any_ready } => {
                let ready = !rx.is_empty();
                Err(State::IsReady {
                    any_ready: any_ready || ready,
                })
            }
            State::Unsubscribe { id_woken } => {
                if id_woken != 0 {
                    rx.as_channel().monitor().watch_abort();
                }
                Err(State::Unsubscribe {
                    id_woken: if id_woken == 0 && !rx.as_channel().monitor().watch_stop() {
                        id(rx)
                    } else {
                        id_woken
                    },
                })
            }
            State::FinalTryRecv { id_woken } => {
                if id(rx) == id_woken {
                    if let Ok(t) = rx.try_recv() {
                        return Ok(t);
                    }
                }
                Err(State::FinalTryRecv { id_woken })
            }
            State::TimedOut => Err(State::TimedOut),
            State::Disconnected => Err(State::Disconnected),
        }
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
