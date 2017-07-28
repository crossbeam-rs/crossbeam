use std::marker::PhantomData;
use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::{Duration, Instant};

use rand::{Rng, thread_rng};

use {Flavor, Sender, Receiver};
use actor::{self, ACTOR, Actor};
use err::{TryRecvError, TrySendError};
use watch::dock::Request;

// TODO: What if send and receive go on the same channel? perhaps sender/receiver for the same
// channel should have different ids!!!!

pub struct Select {
    machine: Machine,
    _marker: PhantomData<*mut ()>,
}

impl Select {
    #[inline]
    pub fn new() -> Self {
        Select::with_deadline(None)
    }

    #[inline]
    pub fn with_timeout(dur: Duration) -> Self {
        Select::with_deadline(Some(Instant::now() + dur))
    }

    #[inline]
    fn with_deadline(deadline: Option<Instant>) -> Self {
        Select {
            machine: Machine::Counting {
                len: 0,
                id_first: 0,
                deadline,
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

    pub fn send<T>(&mut self, tx: &Sender<T>, mut value: T) -> Result<(), T> {
        if let Some(state) = self.machine.step(tx.id()) {
            state.send(tx, value)
        } else {
            Err(value)
        }
    }

    pub fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        if let Some(state) = self.machine.step(rx.id()) {
            state.recv(rx)
        } else {
            Err(())
        }
    }
}

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
    #[inline]
    fn step(&mut self, id: usize) -> Option<&mut State> {
        loop {
            match *self {
                Machine::Counting {
                    len,
                    id_first,
                    deadline,
                } => {
                    if id_first == id {
                        *self = Machine::Initialized {
                            pos: 0,
                            state: State::Try { any_open: false },
                            len,
                            start: thread_rng().gen_range(0, len),
                            deadline,
                        };
                    } else {
                        *self = Machine::Counting {
                            len: len + 1,
                            id_first: if id_first == 0 { id } else { id_first },
                            deadline,
                        };
                        return None;
                    }
                }
                Machine::Initialized {
                    pos,
                    mut state,
                    len,
                    start,
                    deadline,
                } => {
                    if pos >= 2 * len {
                        state.transition(deadline);
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
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Try { any_open: bool },
    Subscribe,
    IsReady,
    Unsubscribe,
    FinalTry,
    TimedOut,
    Disconnected,
}

impl State {
    #[inline]
    fn transition(&mut self, deadline: Option<Instant>) {
        match *self {
            State::Try { any_open } => {
                if any_open {
                    actor::reset();
                    *self = State::Subscribe;
                } else {
                    *self = State::Disconnected;
                }
            }
            State::Subscribe => *self = State::IsReady,
            State::IsReady => {
                actor::wait_until(deadline);
                *self = State::Unsubscribe;
            }
            State::Unsubscribe => *self = State::FinalTry, // TODO: before going to finaltry we should reset and use FinalTry { selected_id: usize }
            State::FinalTry => {
                *self = State::Try { any_open: false };

                if let Some(end) = deadline {
                    if Instant::now() >= end {
                        *self = State::TimedOut;
                    }
                }
            }
            State::TimedOut => {}
            State::Disconnected => {}
        }
    }

    fn send<T>(&mut self, tx: &Sender<T>, mut value: T) -> Result<(), T> {
        match *self {
            State::Try { ref mut any_open } => {
                match tx.try_send(value) {
                    Ok(()) => return Ok(()),
                    Err(TrySendError::Disconnected(v)) => value = v,
                    Err(TrySendError::Full(v)) => {
                        value = v;
                        *any_open = true;
                    }
                }
            }
            State::Subscribe => {
                match tx.0.flavor {
                    // TODO: rename to monitor_senders? mon_senders? send_monitor?
                    Flavor::List(ref q) => {},
                    Flavor::Array(ref q) => q.monitor_tx().register(),
                    Flavor::Zero(ref q) => unimplemented!(),//q.promise_send(),
                }
            }
            State::IsReady => {
                if tx.is_disconnected() || !tx.is_full() {
                    actor::current().select(1);
                }
            }
            State::Unsubscribe => {
                match tx.0.flavor {
                    // TODO: rename to monitor_send?
                    Flavor::List(ref q) => {},
                    Flavor::Array(ref q) => q.monitor_tx().unregister(),
                    Flavor::Zero(ref q) => unimplemented!(),//q.unpromise_send(),
                }
            }
            State::FinalTry => {
                if tx.id() == actor::selected() {
                    match tx.0.flavor {
                        Flavor::Array(..) | Flavor::List(..) => {
                            match tx.try_send(value) {
                                Ok(()) => return Ok(()),
                                Err(TrySendError::Full(v)) => value = v,
                                Err(TrySendError::Disconnected(v)) => value = v,
                            }
                        }
                        Flavor::Zero(ref q) => {
                            unimplemented!(); //actor::request_put(tx.id());
                            return Ok(());
                        }
                    }
                }
            }
            State::TimedOut => {}
            State::Disconnected => {}
        }
        Err(value)
    }

    fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        match *self {
            State::Try { ref mut any_open } => {
                match rx.try_recv() {
                    Ok(t) => return Ok(t),
                    Err(TryRecvError::Disconnected) => {}
                    Err(TryRecvError::Empty) => *any_open = true,
                }
            }
            State::Subscribe => {
                match rx.0.flavor {
                    Flavor::List(ref q) => q.monitor_rx().register(),
                    Flavor::Array(ref q) => q.monitor_rx().register(),
                    Flavor::Zero(ref q) => q.promise_recv(),
                }
            }
            State::IsReady => {
                if rx.is_disconnected() || !rx.is_empty() {
                    actor::current().select(1);
                }
            }
            State::Unsubscribe => {
                match rx.0.flavor {
                    Flavor::List(ref q) => q.monitor_rx().unregister(),
                    Flavor::Array(ref q) => q.monitor_rx().unregister(),
                    Flavor::Zero(ref q) => q.unpromise_recv(),
                }
            }
            State::FinalTry => {
                if rx.id() == actor::selected() {
                    match rx.0.flavor {
                        Flavor::Array(..) | Flavor::List(..) => {
                            if let Ok(t) = rx.try_recv() {
                                return Ok(t);
                            }
                        }
                        Flavor::Zero(ref q) => return Ok(actor::request_take(rx.id())),
                    }
                }
            }
            State::TimedOut => {}
            State::Disconnected => {}
        }
        Err(())
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
                // let mut s = Select::new();
                let mut s = Select::with_timeout(ms(100));
                loop {
                    if let Ok(x) = s.recv(&rx1) {
                        println!("{}", x);
                        break;
                    }
                    if let Ok(x) = s.recv(&rx2) {
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
    //             // let mut s = Select::new();
    //             let mut s = Select::with_timeout(ms(100));
    //             loop {
    //                 if let Ok(()) = s.send(&tx1, 1) {
    //                     println!("sent 1");
    //                     break;
    //                 }
    //                 if let Ok(()) = s.send(&tx2, 2) {
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
