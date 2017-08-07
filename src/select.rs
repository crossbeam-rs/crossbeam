use std::cell::Cell;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use {Sender, Receiver};
use actor::{self, HandleId};
use err::{TryRecvError, TrySendError};
use Backoff;

#[thread_local]
static mut MACHINE: Select = Select::new();

// thread_local! {
//     static MACHINE: Cell<Select> = Cell::new(Select::new());
// }

#[inline]
pub fn blocked() -> bool {
    // MACHINE.with(|m| {
    unsafe {
        let mut t = MACHINE.clone();
        if t.is_blocked() {
            MACHINE = Select::new();
            true
        } else {
            MACHINE = t;
            false
        }
    }
    // })
}

#[inline]
pub fn send<T>(tx: &Sender<T>, value: T) -> Result<(), T> {
    // MACHINE.with(|m| {
        // let mut t = m.get();
    unsafe {
        let mut t = MACHINE.clone();
        let res = t.send(tx, value);
        MACHINE = t;
        res
    }
    // })
}

#[inline]
pub fn recv<T>(rx: &Receiver<T>) -> Result<T, ()> {
    // MACHINE.with(|m| {
    unsafe {
        let mut t = MACHINE.clone();
        let res = t.recv(rx);
        MACHINE = t;
        res
    }
    // })
}

#[derive(Clone, Copy)]
pub struct Select {
    machine: Machine,
    _marker: PhantomData<*mut ()>,
}

impl Select {
    #[inline]
    pub const fn new() -> Self {
        Select::with_deadline(None)
    }

    #[inline]
    pub fn with_timeout(dur: Duration) -> Self {
        Select::with_deadline(Some(Instant::now() + dur))
    }

    #[inline]
    const fn with_deadline(deadline: Option<Instant>) -> Self {
        Select {
            machine: Machine::Counting {
                len: 0,
                first_id: HandleId::sentinel(),
                deadline,
                has_blocked_case: false,
            },
            _marker: PhantomData,
        }
    }

    pub fn send<T>(&mut self, tx: &Sender<T>, value: T) -> Result<(), T> {
        let res = if let Some(state) = self.machine.step(tx.id()) {
            state.send(tx, value)
        } else {
            Err(value)
        };
        if res.is_ok() {
            *self = Select::new();
        }
        res
    }

    pub fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        let res = if let Some(state) = self.machine.step(rx.id()) {
            state.recv(rx)
        } else {
            Err(())
        };
        if res.is_ok() {
            *self = Select::new();
        }
        res
    }

    #[inline]
    pub fn is_disconnected(&mut self) -> bool {
        if let Machine::Initialized { state, .. } = self.machine {
            if let State::Disconnected = state {
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn is_blocked(&mut self) -> bool {
        match self.machine {
            Machine::Counting {
                ref mut has_blocked_case,
                ..
            } => {
                *has_blocked_case = true;
                false
            }
            Machine::Initialized { state, .. } => state == State::Blocked,
        }
    }

    #[inline]
    pub fn timed_out(&mut self) -> bool {
        if let Machine::Initialized { state, .. } = self.machine {
            if let State::TimedOut = state {
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Copy)]
enum Machine {
    Counting {
        len: usize,
        first_id: HandleId,
        deadline: Option<Instant>,
        has_blocked_case: bool,
    },
    Initialized {
        pos: usize,
        state: State,
        len: usize,
        start: usize,
        deadline: Option<Instant>,
        has_blocked_case: bool,
    },
}

impl Machine {
    #[inline]
    fn step(&mut self, id: HandleId) -> Option<&mut State> {
        loop {
            match *self {
                Machine::Counting {
                    len,
                    first_id,
                    deadline,
                    has_blocked_case,
                } => {
                    if first_id == HandleId::sentinel() {
                        start_selection();
                    }

                    if first_id == id {
                        *self = Machine::Initialized {
                            pos: 0,
                            state: State::Try { closed_count: 0 },
                            len,
                            start: gen_random(len),
                            deadline,
                            has_blocked_case,
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
                            has_blocked_case,
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
                    has_blocked_case,
                } => {
                    if pos >= 2 * len {
                        state.transition(len, deadline, has_blocked_case);
                        *self = Machine::Initialized {
                            pos: 0,
                            state,
                            len,
                            start,
                            deadline,
                            has_blocked_case,
                        };
                    } else {
                        *self = Machine::Initialized {
                            pos: pos + 1,
                            state,
                            len,
                            start,
                            deadline,
                            has_blocked_case,
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
    Try { closed_count: usize },
    Register { closed_countdown: isize },
    Unregister,
    FinalTry { id: HandleId },
    Finished,
    Disconnected,
    Blocked,
    TimedOut,
}

impl State {
    #[inline]
    fn transition(&mut self, len: usize, deadline: Option<Instant>, has_blocked_case: bool) {
        match *self {
            State::Try { closed_count } => {
                if closed_count < len {
                    actor::current().reset();

                    if has_blocked_case {
                        *self = State::Blocked;
                        end_selection();
                    } else {
                        *self = State::Register {
                            closed_countdown: closed_count as isize,
                        };
                    }
                } else {
                    *self = State::Disconnected;
                    end_selection();
                }
            }
            State::Register { closed_countdown } => {
                if closed_countdown < 0 {
                    actor::current().select(HandleId::sentinel());
                }
                actor::current().wait_until(deadline);
                *self = State::Unregister;
            }
            State::Unregister => {
                *self = State::FinalTry {
                    id: actor::current().selected(),
                };
            }
            State::FinalTry { .. } => {
                *self = State::Try { closed_count: 0 };

                if let Some(end) = deadline {
                    if Instant::now() >= end {
                        *self = State::TimedOut;
                        end_selection();
                    }
                }
            }
            State::Finished => {}
            State::Disconnected => {}
            State::Blocked => {}
            State::TimedOut => {}
        }
    }

    fn send<T>(&mut self, tx: &Sender<T>, mut value: T) -> Result<(), T> {
        match *self {
            State::Try {
                ref mut closed_count,
            } => {
                let backoff = &mut Backoff::new();
                loop {
                    match tx.try_send_with_backoff(value, backoff) {
                        Ok(()) => return Ok(()),
                        Err(TrySendError::Full(v)) => value = v,
                        Err(TrySendError::Disconnected(v)) => {
                            value = v;
                            *closed_count += 1;
                            break;
                        }
                    }
                    if !backoff.tick() {
                        break;
                    }
                }
            }
            State::Register {
                ref mut closed_countdown,
            } => {
                tx.register();
                if tx.is_disconnected() {
                    *closed_countdown -= 1;
                }
                if tx.can_send() {
                    actor::current().select(HandleId::sentinel());
                }
            }
            State::Unregister => tx.unregister(),
            State::FinalTry { id } => {
                if tx.id() == id {
                    match tx.send_promised(value) {
                        Ok(()) => return Ok(()),
                        Err(v) => value = v,
                    }
                }
            }
            State::Finished => {}
            State::Disconnected => {}
            State::Blocked => {}
            State::TimedOut => {}
        }
        Err(value)
    }

    fn recv<T>(&mut self, rx: &Receiver<T>) -> Result<T, ()> {
        match *self {
            State::Try {
                ref mut closed_count,
            } => {
                let backoff = &mut Backoff::new();
                loop {
                    match rx.try_recv_with_backoff(backoff) {
                        Ok(v) => return Ok(v),
                        Err(TryRecvError::Empty) => {}
                        Err(TryRecvError::Disconnected) => {
                            *closed_count += 1;
                            break;
                        }
                    }
                    if !backoff.tick() {
                        break;
                    }
                }
            }
            State::Register {
                ref mut closed_countdown,
            } => {
                rx.register();
                if rx.is_disconnected() {
                    *closed_countdown -= 1;
                }
                if rx.can_recv() {
                    actor::current().select(HandleId::sentinel());
                }
            }
            State::Unregister => rx.unregister(),
            State::FinalTry { id } => {
                if rx.id() == id {
                    if let Ok(v) = rx.recv_promised() {
                        return Ok(v);
                    }
                }
            }
            State::Finished => {}
            State::Disconnected => {}
            State::Blocked => {}
            State::TimedOut => {}
        }
        Err(())
    }
}

thread_local! {
    static SELECTING: Cell<bool> = Cell::new(false);
    static RNG: Cell<u32> = Cell::new(1);
}

fn start_selection() {
    // SELECTING.with(|s| {
    //     if s.get() {
    //         panic!("Illegal use of `Select` - multiple selections running at the same time!");
    //     }
    //     s.set(true);
    // });
}

fn end_selection() {
    // SELECTING.with(|s| s.set(false));
}

fn gen_random(ceil: usize) -> usize {
    RNG.with(|rng| {
        let mut x = ::std::num::Wrapping(rng.get());
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rng.set(x.0);

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

    use {Select, bounded, unbounded};
    use err::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn smoke1() {
        let (tx1, rx1) = unbounded::<i32>();
        let (tx2, rx2) = unbounded::<i32>();
        tx1.send(1).unwrap();

        let mut s = Select::new();
        loop {
            if let Ok(v) = s.recv(&rx1) {
                assert_eq!(v, 1);
                break;
            }
            if let Ok(_) = s.recv(&rx2) {
                panic!();
                break;
            }
        }

        tx2.send(2).unwrap();

        let mut s = Select::new();
        loop {
            if let Ok(_) = s.recv(&rx1) {
                panic!();
                break;
            }
            if let Ok(v) = s.recv(&rx2) {
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

        let mut s = Select::new();
        loop {
            if let Ok(_) = s.recv(&rx1) {
                panic!();
                break;
            }
            if let Ok(_) = s.recv(&rx2) {
                panic!();
                break;
            }
            if s.is_disconnected() {
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
                    if s.is_disconnected() {
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
                // let mut s = Select::new();
                let mut s = Select::with_timeout(ms(100));
                loop {
                    if let Ok(()) = s.send(&tx1, 1) {
                        println!("sent 1");
                        break;
                    }
                    if let Ok(()) = s.send(&tx2, 2) {
                        println!("sent 2");
                        break;
                    }
                    if s.is_disconnected() {
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
