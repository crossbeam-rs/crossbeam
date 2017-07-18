use std::collections::VecDeque;
use std::ptr;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool, AtomicPtr, AtomicUsize};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release, Relaxed, SeqCst};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use rand::{Rng, thread_rng};

enum State {
    Count(*const Monitor),
    TryRecv1,
    TryRecv2,
    Subscribe,
    IsReady(bool),
    Unsubscribe,
}

pub struct Select {
    state: State,
    pos: usize,
    len: usize,
    start: usize,
    closed_count: usize,
    timed_out: bool,
    disconnected: bool,
    deadline: Option<Instant>,
}

impl Select {
    pub fn new() -> Self {
        Select {
            state: State::Count(ptr::null()),
            pos: 0,
            len: 0,
            start: 0,
            closed_count: 0,
            timed_out: false,
            disconnected: false,
            deadline: None,
        }
    }

    pub fn with_timeout(dur: Duration) -> Self {
        Select {
            state: State::Count(ptr::null()),
            pos: 0,
            len: 0,
            start: 0,
            closed_count: 0,
            timed_out: false,
            disconnected: false,
            deadline: Some(Instant::now() + dur),
        }
    }

    pub fn timed_out(&self) -> bool {
        self.timed_out
    }

    pub fn disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn poll<T>(&mut self, q: &Queue<T>) -> Option<T> {
        loop {
            match self.state {
                State::Count(fst) => {
                    if fst == q.receivers() {
                        self.start = thread_rng().gen_range(0, self.len);
                        self.state = State::TryRecv1;
                        self.pos = 0;
                        self.closed_count = 0;
                    } else {
                        if fst.is_null() {
                            self.state = State::Count(q.receivers());
                        }
                        self.pos += 1;
                        self.len += 1;
                        return None;
                    }
                }
                State::TryRecv1 => {
                    if self.pos == self.len {
                        self.state = State::TryRecv2;
                        self.pos = 0;
                    } else {
                        self.pos += 1;
                        if self.pos > self.start && !self.timed_out {
                            match q.try_recv() {
                                Ok(v) => return Some(v),
                                Err(TryRecvError::Disconnected) => self.closed_count += 1,
                                _ => {}
                            }
                        }
                        return None;
                    }
                }
                State::TryRecv2 => {
                    if self.pos == self.len {
                        self.state = State::Subscribe;
                        self.pos = 0;
                    } else {
                        self.pos += 1;
                        if self.pos <= self.start && !self.timed_out {
                            match q.try_recv() {
                                Ok(v) => return Some(v),
                                Err(TryRecvError::Disconnected) => self.closed_count += 1,
                                _ => {}
                            }
                        }
                        return None;
                    }
                }
                State::Subscribe => {
                    if self.pos == self.len {
                        self.state = State::IsReady(false);
                        self.pos = 0;
                    } else {
                        self.pos += 1;
                        if !self.disconnected {
                            q.receivers().subscribe();
                        }
                    }
                }
                State::IsReady(ready) => {
                    if self.pos == self.len {
                        if !ready {
                            let now = Instant::now();
                            if let Some(end) = self.deadline {
                                if now < end {
                                    thread::park_timeout(end - now);
                                }
                            } else {
                                thread::park();
                            }
                        }
                        self.state = State::Unsubscribe;
                        self.pos = 0;
                    } else {
                        self.pos += 1;
                        if !self.disconnected && q.is_ready() {
                            self.state = State::IsReady(true);
                        }
                    }
                }
                State::Unsubscribe => {
                    if self.pos == self.len {
                        if !self.timed_out {
                            if let Some(end) = self.deadline {
                                if Instant::now() > end {
                                    self.timed_out = true;
                                }
                            }
                        }

                        if self.closed_count == self.len {
                            self.disconnected = true;
                        }

                        self.state = State::TryRecv1;
                        self.pos = 0;
                        self.closed_count = 0;
                    } else {
                        self.pos += 1;
                        if !self.disconnected {
                            q.receivers().unsubscribe();
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn select() {
        let a = Queue::sync(1000);
        let b = Queue::sync(1000);

        crossbeam::scope(|s| {
            s.spawn(|| {
                thread::sleep(ms(100));
                a.send(1);
                b.send(2);
                a.close();
                b.close();
            });
            s.spawn(|| {
                // let mut s = Select::new();
                let mut s = Select::with_timeout(ms(110));
                loop {
                    if let Some(x) = s.poll(&a) {
                        println!("{}", x);
                        break;
                    }
                    if let Some(x) = s.poll(&b) {
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
