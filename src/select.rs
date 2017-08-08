use std::cell::Cell;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use {Sender, Receiver};
use actor::{self, HandleId};
use err::{TryRecvError, TrySendError};

thread_local! {
    static MACHINE: Cell<Machine> = Cell::new(Machine::new());
}

pub fn send<T>(tx: &Sender<T>, value: T) -> Result<(), T> {
    MACHINE.with(|m| {
        let mut t = m.get();

        let res = if let Some(state) = t.step(tx.id()) {
            state.send(tx, value)
        } else {
            Err(value)
        };
        if res.is_ok() {
            t = Machine::new();
        }

        m.set(t);
        res
    })
}

pub fn recv<T>(rx: &Receiver<T>) -> Result<T, ()> {
    MACHINE.with(|m| {
        let mut t = m.get();

        let res = if let Some(state) = t.step(rx.id()) {
            state.recv(rx)
        } else {
            Err(())
        };
        if res.is_ok() {
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
                } => {
                    if first_id == id {
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
                }
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
            State::TryOnce { closed_count } => {
                if closed_count < len {
                    *self = State::Blocked;
                } else {
                    *self = State::Disconnected;
                }
            }
            State::SpinTry { closed_count } => {
                if closed_count < len {
                    *self = State::Promise { closed_count: 0 };
                } else {
                    *self = State::Disconnected;
                }
            }
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
            }
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
            State::Fulfill { id } => {
                if tx.id() == id {
                    match tx.fulfill_send(value) {
                        Ok(()) => return Ok(()),
                        Err(v) => value = v,
                    }
                }
            }
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
            } => {
                match rx.try_recv() {
                    Ok(v) => return Ok(v),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => *closed_count += 1,
                }
            }
            State::SpinTry {
                ref mut closed_count,
            } => {
                match rx.spin_try_recv() {
                    Ok(v) => return Ok(v),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => *closed_count += 1,
                }
            }
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
            State::Fulfill { id } => {
                if rx.id() == id {
                    if let Ok(v) = rx.fulfill_recv() {
                        return Ok(v);
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use crossbeam;

    use {bounded, unbounded, select};
    use err::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn smoke1() {
        let (tx1, rx1) = unbounded::<i32>();
        let (tx2, rx2) = unbounded::<i32>();
        tx1.send(1).unwrap();

        loop {
            if let Ok(v) = rx1.select() {
                assert_eq!(v, 1);
                break;
            }
            if let Ok(_) = rx2.select() {
                panic!();
            }
        }

        tx2.send(2).unwrap();

        loop {
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(v) = rx2.select() {
                assert_eq!(v, 2);
                break;
            }
        }
    }

    #[test]
    fn disconnected() {
        let (tx1, rx1) = unbounded::<i32>();
        let (tx2, rx2) = unbounded::<i32>();

        drop(tx1);
        drop(tx2);

        loop {
            if let Ok(_) = rx1.select() {
                panic!();
            }
            if let Ok(_) = rx2.select() {
                panic!();
            }
            if select::disconnected() {
                break;
            }
        }
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
                loop {
                    if let Ok(x) = rx1.select() {
                        println!("{}", x);
                        break;
                    }
                    if let Ok(x) = rx2.select() {
                        println!("{}", x);
                        break;
                    }
                    if select::disconnected() {
                        println!("DISCONNECTED!");
                        break;
                    }
                    if select::timeout(ms(100)) {
                        println!("TIMEOUT!");
                        break;
                    }
                }
                drop(rx1);
                drop(rx2);
            });
        });
    }

    #[test]
    fn select_send() {
        let (tx1, rx1) = bounded::<i32>(0);
        let (tx2, rx2) = bounded::<i32>(0);

        crossbeam::scope(|s| {
            s.spawn(|| {
                thread::sleep(ms(150));
                println!("got 1: {:?}", rx1.recv_timeout(ms(200)));
            });
            s.spawn(|| {
                thread::sleep(ms(150));
                println!("got 2: {:?}", rx2.recv_timeout(ms(200)));
            });
            s.spawn(|| {
                thread::sleep(ms(100));
                loop {
                    if let Ok(()) = tx1.select(1) {
                        println!("sent 1");
                        break;
                    }
                    if let Ok(()) = tx2.select(2) {
                        println!("sent 2");
                        break;
                    }
                    if select::disconnected() {
                        println!("DISCONNECTED!");
                        break;
                    }
                    if select::timeout(ms(100)) {
                        println!("TIMEOUT!");
                        break;
                    }
                }
            });
        });
    }
}
