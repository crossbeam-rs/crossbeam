use std::marker::PhantomData;
use std::thread;
use std::time::{Duration, Instant};

use rand::{Rng, thread_rng};

use err::TryRecvError;
use channel::Receiver;

// TODO: select should unsubscribe, and everyone else should cancel
// TODO: alternative terminology: watch/unwatch/abort
// TODO: impl !Send and !Sync

enum State {
    Count(usize),
    TryRecv1,
    TryRecv2,
    Subscribe,
    IsReady(bool),
    Unsubscribe,
    FinalTryRecv,
    Finished,
}

pub struct Select {
    state: State,
    pos: usize,
    len: usize,
    start: usize,
    woken: usize,
    closed_count: usize,
    timed_out: bool,
    disconnected: bool,
    deadline: Option<Instant>,
    _marker: PhantomData<*mut ()>,
}

impl Select {
    #[inline]
    pub fn new() -> Self {
        Select {
            state: State::Count(0),
            pos: 0,
            len: 0,
            start: 0,
            woken: !0,
            closed_count: 0,
            timed_out: false,
            disconnected: false,
            deadline: None,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn with_timeout(dur: Duration) -> Self {
        Select {
            state: State::Count(0),
            pos: 0,
            len: 0,
            start: 0,
            woken: !0,
            closed_count: 0,
            timed_out: false,
            disconnected: false,
            deadline: Some(Instant::now() + dur),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn timed_out(&self) -> bool {
        self.timed_out
    }

    #[inline]
    pub fn disconnected(&self) -> bool {
        self.disconnected
    }

    pub fn poll<T>(&mut self, rx: &Receiver<T>) -> Option<T> {
        let chan = rx.as_channel();
        // println!("POLL {:?}", rx as *const _);

        loop {
            match self.state {
                State::Count(first) => {
                    if first == chan.id() {
                        self.start = thread_rng().gen_range(0, self.len);
                        self.state = State::TryRecv1;
                        self.pos = 0;
                        self.closed_count = 0;
                    } else {
                        if first == 0 {
                            self.state = State::Count(chan.id());
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
                        if self.pos >= self.start {
                            // println!("TRY RECV 1 {}", self.pos);
                            match chan.try_recv() {
                                Ok(v) => return Some(v),
                                Err(TryRecvError::Disconnected) => self.closed_count += 1,
                                _ => {}
                            }
                        }
                        self.pos += 1;
                        return None;
                    }
                }
                State::TryRecv2 => {
                    if self.pos == self.len {
                        self.state = State::Subscribe;
                        self.pos = 0;
                    } else {
                        if self.pos < self.start {
                            // println!("TRY RECV 2 {}", self.pos);
                            match chan.try_recv() {
                                Ok(v) => return Some(v),
                                Err(TryRecvError::Disconnected) => self.closed_count += 1,
                                _ => {}
                            }
                        }
                        self.pos += 1;
                        return None;
                    }
                }
                State::Subscribe => {
                    if self.pos == self.len {
                        self.state = State::IsReady(false);
                        self.pos = 0;
                    } else {
                        if !self.disconnected {
                            // println!("SUBSCRIBE {} {:?}", self.pos, rx as *const _);
                            chan.monitor().watch_start();
                        }
                        self.pos += 1;
                        return None;
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
                        self.state = State::Unsubscribe; // TODO: This should be WatchAbort
                        self.pos = 0;
                        self.woken = !0;
                    } else {
                        if !self.disconnected && chan.is_ready() {
                            self.state = State::IsReady(true);
                        }
                        self.pos += 1;
                        return None;
                    }
                }
                State::Unsubscribe => {
                    if self.pos == self.len {
                        self.state = State::FinalTryRecv;
                        self.pos = 0;
                    } else {
                        // println!("UNSUBSCRIBE {}", self.pos);
                        if self.woken == !0 {
                            if !chan.monitor().watch_stop() {
                                self.woken = self.pos;
                            }
                        } else {
                            chan.monitor().watch_abort();
                        }
                        self.pos += 1;
                        return None;
                    }
                }
                State::FinalTryRecv => {
                    if self.pos == self.len {
                        if let Some(end) = self.deadline {
                            if Instant::now() >= end {
                                self.timed_out = true;
                            }
                        }

                        if self.closed_count == self.len {
                            self.disconnected = true;
                        }

                        if self.timed_out || self.disconnected {
                            self.state = State::Finished;
                        } else {
                            self.state = State::TryRecv1;
                            self.pos = 0;
                            self.closed_count = 0;
                        }
                    } else {
                        if !self.disconnected && !self.timed_out && self.woken == self.pos {
                            // println!("FINAL TRY RECV {}", self.pos);
                            if let Ok(v) = chan.try_recv() {
                                return Some(v);
                            }
                            chan.monitor().notify_one();
                        }
                        self.pos += 1;
                        return None;
                    }
                }
                State::Finished => {
                    return None;
                }
            }
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
