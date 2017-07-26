use std::marker::PhantomData;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::{Duration, Instant};

use rand::{Rng, thread_rng};

use Receiver;
use Sender;
use err::{TryRecvError, TrySendError};
use impls::Channel;
use actor::{self, ACTOR, Actor, Request};
use Flavor;

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

    // pub fn poll_tx<T>(&mut self, tx: &Sender<T>, value: T) -> Result<(), T> {
    //     match self.machine.poll_tx(tx, value) {
    //         Ok(()) => Ok(()),
    //         Err((v, m)) => {
    //             self.machine = m;
    //             Err(v)
    //         }
    //     }
    // }

    pub fn poll_rx<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        match self.machine.poll_rx(rx) {
            Ok(t) => Ok(t),
            Err(m) => {
                self.machine = m;
                Err(())
            }
        }
    }
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
    fn poll_rx<T>(mut self, rx: &Receiver<T>) -> Result<T, Machine> {
        let id = rx.as_channel() as *const Channel<T> as *const u8 as usize;
        loop {
            self = match self {
                Machine::Counting {
                    len,
                    id_first,
                    deadline,
                } => {
                    if id_first == id {
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
                            id_first: if id_first == 0 { id } else { id_first },
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
                        match state.poll_rx(rx) {
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
    IsReady,
    Unsubscribe,
    FinalTryRecv,
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
                    actor::reset();
                    ACTOR.with(|a| a.request_ptr.store(0, SeqCst));
                    State::Subscribe
                }
            }
            State::Subscribe => State::IsReady,
            State::IsReady => {
                actor::wait_until(deadline);
                State::Unsubscribe
            }
            State::Unsubscribe => {
                // TODO: final try only if sel != 1
                State::FinalTryRecv
            }
            State::FinalTryRecv => {
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

    fn poll_rx<T>(self, rx: &Receiver<T>) -> Result<T, State> {
        let id = rx.as_channel() as *const Channel<T> as *const u8 as usize;
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
                match rx.0.flavor {
                    Flavor::List(ref q) => q.monitor_rx().register(),
                    Flavor::Array(ref q) => q.monitor_rx().register(),
                    Flavor::Zero(ref q) => q.promise_recv(),
                }
                Err(State::Subscribe)
            }
            State::IsReady => {
                if !rx.is_empty() {
                    // TODO: || rx.is_closed() {
                    ACTOR.with(|a| a.select_id.compare_and_swap(0, 1, SeqCst));
                }
                Err(State::IsReady)
            }
            State::Unsubscribe => {
                match rx.0.flavor {
                    Flavor::List(ref q) => q.monitor_rx().unregister(),
                    Flavor::Array(ref q) => q.monitor_rx().unregister(),
                    Flavor::Zero(ref q) => q.unpromise_recv(),
                }
                Err(State::Unsubscribe)
            }
            State::FinalTryRecv => {
                if id == ACTOR.with(|a| a.select_id.load(SeqCst)) {
                    match rx.0.flavor {
                        Flavor::Array(..) | Flavor::List(..) => {
                            if let Ok(t) = rx.try_recv() {
                                return Ok(t);
                            }
                        }
                        Flavor::Zero(ref q) => {
                            let req =
                                ACTOR.with(|a| a.request_ptr.swap(0, SeqCst)) as *const Request<T>;
                            assert!(!req.is_null());

                            unsafe {
                                let thread = (*req).actor.thread.clone();
                                let v = (*(*req).data.get()).take().unwrap();
                                (*req).actor.select_id.store(id, SeqCst);
                                thread.unpark();
                                return Ok(v);
                            }
                        }
                    }
                }
                Err(State::FinalTryRecv)
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
    fn select_recv() {
        let (tx1, rx1) = bounded::<i32>(0);
        let (tx2, rx2) = bounded::<i32>(0);

        crossbeam::scope(|s| {
            s.spawn(|| {
                // thread::sleep(ms(150));
                // tx1.send_timeout(1, ms(200));
                loop {
                    match tx1.try_send(1) {
                        Ok(()) => break,
                        Err(TrySendError::Disconnected(_)) => break,
                        Err(TrySendError::Full(_)) => continue,
                    }
                }
            });
            s.spawn(|| {
                // thread::sleep(ms(150));
                // tx2.send_timeout(2, ms(200));
                loop {
                    match tx2.try_send(2) {
                        Ok(()) => break,
                        Err(TrySendError::Disconnected(_)) => break,
                        Err(TrySendError::Full(_)) => continue,
                    }
                }
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                // let s = &mut Select::new();
                let s = &mut Select::with_timeout(ms(100));
                loop {
                    if let Ok(x) = rx1.poll_recv(s) {
                        println!("{}", x);
                        break;
                    }
                    if let Ok(x) = rx2.poll_recv(s) {
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
                drop(rx1);
                drop(rx2);
            });
        });
    }

    // #[test]
    // fn select_send() {
    //     let (tx1, rx1) = bounded::<i32>(0);
    //     let (tx2, rx2) = bounded::<i32>(0);
    //
    //     crossbeam::scope(|s| {
    //         s.spawn(|| {
    //             thread::sleep(ms(150));
    //             println!("got 1: {:?}", rx1.recv_timeout(ms(200)));
    //         });
    //         s.spawn(|| {
    //             thread::sleep(ms(150));
    //             println!("got 2: {:?}", rx2.recv_timeout(ms(200)));
    //         });
    //         s.spawn(|| {
    //             thread::sleep(ms(100));
    //             // let s = &mut Select::new();
    //             let s = &mut Select::with_timeout(ms(100));
    //             loop {
    //                 if let Ok(()) = tx1.poll_send(1, s) {
    //                     println!("sent 1");
    //                     break;
    //                 }
    //                 if let Ok(()) = tx2.poll_send(2, s) {
    //                     println!("sent 2");
    //                     break;
    //                 }
    //                 if s.disconnected() {
    //                     println!("DISCONNECTED!");
    //                     break;
    //                 }
    //                 if s.timed_out() {
    //                     println!("TIMEOUT!");
    //                     break;
    //                 }
    //             }
    //         });
    //     });
    // }
}
