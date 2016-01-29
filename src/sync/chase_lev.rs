// Copyright 2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A lock-free concurrent work-stealing deque
//!
//! This module contains an implementation of the Chase-Lev work stealing deque
//! described in "Dynamic Circular Work-Stealing Deque". The implementation is
//! heavily based on the pseudocode found in the paper.
//!
//! # Example
//!
//! ```
//! use crossbeam::sync::chase_lev;
//! let (mut worker, stealer) = chase_lev::deque();
//!
//! // Only the worker may push/try_pop
//! worker.push(1);
//! worker.try_pop();
//!
//! // Stealers take data from the other end of the deque
//! worker.push(1);
//! stealer.steal();
//!
//! // Stealers can be cloned to have many stealers stealing in parallel
//! worker.push(1);
//! let stealer2 = stealer.clone();
//! stealer2.steal();
//! ```

// FIXME: all atomic operations in this module use a SeqCst ordering. That is
//      probably overkill

use std::cell::UnsafeCell;
use std::mem;
use std::ptr;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::AtomicIsize;
use std::sync::Arc;

use mem::epoch::{self, Atomic, Shared, Owned};

// Once the queue is less than 1/K full, then it will be downsized. Note that
// the deque requires that this number be less than 2.
const K: isize = 4;

// Minimum number of bits that a buffer size should be. No buffer will resize to
// under this value, and all deques will initially contain a buffer of this
// size.
//
// The size in question is 1 << MIN_BITS
const MIN_BITS: u32 = 7;

struct Deque<T> {
    bottom: AtomicIsize,
    top: AtomicIsize,
    array: Atomic<Buffer<T>>,
}

// FIXME: can these constraints be relaxed?
unsafe impl<T: Send> Send for Deque<T> {}
unsafe impl<T: Send> Sync for Deque<T> {}

/// Worker half of the work-stealing deque. This worker has exclusive access to
/// one side of the deque, and uses `push` and `try_pop` method to manipulate it.
///
/// There may only be one worker per deque, and operations on the worker
/// require mutable access to the worker itself.
pub struct Worker<T> {
    deque: Arc<Deque<T>>,
}

/// The stealing half of the work-stealing deque. Stealers have access to the
/// opposite end of the deque from the worker, and they only have access to the
/// `steal` method.
///
/// Stealers can be cloned to have more than one handle active at a time.
pub struct Stealer<T> {
    deque: Arc<Deque<T>>,
}

/// When stealing some data, this is an enumeration of the possible outcomes.
#[derive(PartialEq, Eq, Debug)]
pub enum Steal<T> {
    /// The deque was empty at the time of stealing
    Empty,
    /// The stealer lost the race for stealing data, and a retry may return more
    /// data.
    Abort,
    /// The stealer has successfully stolen some data.
    Data(T),
}

// An internal buffer used by the chase-lev deque. This structure is actually
// implemented as a circular buffer, and is used as the intermediate storage of
// the data in the deque.
//
// This Vec<T> always has a length of 0, the backing buffer is just used by the
// code below.
struct Buffer<T> {
    storage: UnsafeCell<Vec<T>>,
    log_size: u32,
}

impl<T> Worker<T> {
    /// Pushes data onto the front of this work queue.
    pub fn push(&mut self, t: T) {
        unsafe { self.deque.push(t) }
    }

    /// Pops data off the front of the work queue, returning `None` on an empty
    /// queue.
    pub fn try_pop(&mut self) -> Option<T> {
        unsafe { self.deque.try_pop() }
    }
}

impl<T> Stealer<T> {
    /// Steals work off the end of the queue (opposite of the worker's end)
    pub fn steal(&self) -> Steal<T> {
        self.deque.steal()
    }
}

impl<T> Clone for Stealer<T> {
    fn clone(&self) -> Stealer<T> {
        Stealer { deque: self.deque.clone() }
    }
}

/// Creates a new empty deque
pub fn deque<T>() -> (Worker<T>, Stealer<T>) {
    let a = Arc::new(Deque::new());
    let b = a.clone();
    (Worker { deque: a }, Stealer { deque: b })
}

// Almost all of this code can be found directly in the paper so I'm not
// personally going to heavily comment what's going on here.

impl<T> Deque<T> {
    fn new() -> Deque<T> {
        let array = Atomic::null();
        array.store(Some(Owned::new(Buffer::new(MIN_BITS))), SeqCst);
        Deque {
            bottom: AtomicIsize::new(0),
            top: AtomicIsize::new(0),
            array: array,
        }
    }

    unsafe fn push(&self, data: T) {
        let guard = epoch::pin();

        let mut b = self.bottom.load(SeqCst);
        let t = self.top.load(SeqCst);
        let mut a = self.array.load(SeqCst, &guard).unwrap();
        let size = b - t;
        if size >= (a.size() as isize) - 1 {
            // You won't find this code in the chase-lev deque paper. This is
            // alluded to in a small footnote, however. We always free a buffer
            // when growing in order to prevent leaks.
            a = self.swap_buffer(b, a, a.resize(b, t, 1), &guard);
            b = self.bottom.load(SeqCst);
        }
        a.put(b, data);
        self.bottom.store(b + 1, SeqCst);
    }

    unsafe fn try_pop(&self) -> Option<T> {
        let guard = epoch::pin();

        let b = self.bottom.load(SeqCst);
        let a = self.array.load(SeqCst, &guard).unwrap();
        let b = b - 1;
        self.bottom.store(b, SeqCst);
        let t = self.top.load(SeqCst);
        let size = b - t;
        if size < 0 {
            self.bottom.store(t, SeqCst);
            return None
        }
        let data = a.get(b);
        if size > 0 {
            self.maybe_shrink(b, t, &guard);
            return Some(data)
        }
        if self.top.compare_and_swap(t, t + 1, SeqCst) == t {
            self.bottom.store(t + 1, SeqCst);
            return Some(data)
        } else {
            self.bottom.store(t + 1, SeqCst);
            mem::forget(data); // someone else stole this value
            return None
        }
    }

    fn steal(&self) -> Steal<T> {
        let guard = epoch::pin();

        let t = self.top.load(SeqCst);
        let old = self.array.load(SeqCst, &guard).unwrap();
        let b = self.bottom.load(SeqCst);
        let a = self.array.load(SeqCst, &guard).unwrap();
        let size = b - t;
        if size <= 0 {
            return Steal::Empty
        }
        if (size as usize) % a.size() == 0 {
            if *a as *const _ == *old as *const _ && t == self.top.load(SeqCst) {
                return Steal::Empty
            }
            return Steal::Abort
        }
        unsafe {
            let data = a.get(t);
            if self.top.compare_and_swap(t, t + 1, SeqCst) == t {
                Steal::Data(data)
            } else {
                mem::forget(data); // someone else stole this value
                Steal::Abort
            }
        }
    }

    unsafe fn maybe_shrink(&self, b: isize, t: isize, guard: &epoch::Guard) {
        let a = self.array.load(SeqCst, guard).unwrap();
        if b - t < (a.size() as isize) / K && b - t > (1 << MIN_BITS) {
            self.swap_buffer(b, a, a.resize(b, t, -1), guard);
        }
    }

    // Helper routine not mentioned in the paper which is used in growing and
    // shrinking buffers to swap in a new buffer into place.
    //
    // As a bit of a recap, stealers can continue using buffers after this
    // method has called 'unlinked' on it. The continued usage is simply a read
    // followed by a forget, but we must make sure that the memory can continue
    // to be read after we flag this buffer for reclamation. All stealers,
    // however, have their own epoch pinned during this time so the buffer will
    // just naturally be free'd once all concurrent stealers have exited.
    unsafe fn swap_buffer<'a>(&self,
                              b: isize,
                              old: Shared<'a, Buffer<T>>,
                              buf: Buffer<T>,
                              guard: &'a epoch::Guard)
                              -> Shared<'a, Buffer<T>> {
        let newbuf = Owned::new(buf);
        let newbuf = self.array.store_and_ref(newbuf, SeqCst, &guard);
        let ss = newbuf.size() as isize;
        self.bottom.store(b + ss, SeqCst);
        let t = self.top.load(SeqCst);
        if self.top.compare_and_swap(t, t + ss, SeqCst) != t {
            self.bottom.store(b, SeqCst);
        }
        guard.unlinked(old);
        return newbuf
    }
}


impl<T> Drop for Deque<T> {
    fn drop(&mut self) {
        let guard = epoch::pin();

        let t = self.top.load(SeqCst);
        let b = self.bottom.load(SeqCst);
        let a = self.array.swap(None, SeqCst, &guard).unwrap();
        // Free whatever is leftover in the dequeue, then free the backing
        // memory itself
        unsafe {
            for i in t..b {
                drop(a.get(i));
            }
            guard.unlinked(a);
        }
    }
}

impl<T> Buffer<T> {
    fn new(log_size: u32) -> Buffer<T> {
        Buffer {
            storage: UnsafeCell::new(Vec::with_capacity(1 << log_size)),
            log_size: log_size,
        }
    }

    fn size(&self) -> usize {
        unsafe { (*self.storage.get()).capacity() }
    }

    fn mask(&self) -> isize {
        unsafe {
            ((*self.storage.get()).capacity() - 1) as isize
        }
    }

    unsafe fn elem(&self, i: isize) -> *mut T {
        (*self.storage.get()).as_mut_ptr().offset(i & self.mask())
    }

    // This does not protect against loading duplicate values of the same cell,
    // nor does this clear out the contents contained within. Hence, this is a
    // very unsafe method which the caller needs to treat specially in case a
    // race is lost.
    unsafe fn get(&self, i: isize) -> T {
        ptr::read(self.elem(i))
    }

    // Unsafe because this unsafely overwrites possibly uninitialized or
    // initialized data.
    unsafe fn put(&self, i: isize, t: T) {
        ptr::write(self.elem(i), t);
    }

    // Again, unsafe because this has incredibly dubious ownership violations.
    // It is assumed that this buffer is immediately dropped.
    unsafe fn resize(&self, b: isize, t: isize, delta: i32) -> Buffer<T> {
        let buf = Buffer::new(((self.log_size as i32) + delta) as u32);
        for i in t..b {
            buf.put(i, self.get(i));
        }
        return buf;
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use super::{deque, Worker, Stealer, Steal};

    use std::thread;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, ATOMIC_BOOL_INIT,
                            AtomicUsize, ATOMIC_USIZE_INIT};
    use std::sync::atomic::Ordering::SeqCst;

    use self::rand::Rng;

    #[test]
    fn smoke() {
        let (mut w, s) = deque();
        assert_eq!(w.try_pop(), None);
        assert_eq!(s.steal(), Steal::Empty);
        w.push(1);
        assert_eq!(w.try_pop(), Some(1));
        w.push(1);
        assert_eq!(s.steal(), Steal::Data(1));
        w.push(1);
        assert_eq!(s.clone().steal(), Steal::Data(1));
    }

    #[test]
    fn stealpush() {
        static AMT: isize = 100000;
        let (mut w, s) = deque();
        let t = thread::spawn(move || {
            let mut left = AMT;
            while left > 0 {
                match s.steal() {
                    Steal::Data(i) => {
                        assert_eq!(i, 1);
                        left -= 1;
                    }
                    Steal::Abort | Steal::Empty => {}
                }
            }
        });

        for _ in 0..AMT {
            w.push(1);
        }

        t.join().unwrap();
    }

    #[test]
    fn stealpush_large() {
        static AMT: isize = 100000;
        let (mut w, s) = deque();
        let t = thread::spawn(move || {
            let mut left = AMT;
            while left > 0 {
                match s.steal() {
                    Steal::Data((1, 10)) => { left -= 1; }
                    Steal::Data(..) => panic!(),
                    Steal::Abort | Steal::Empty => {}
                }
            }
        });

        for _ in 0..AMT {
            w.push((1, 10));
        }

        t.join().unwrap();
    }

    fn stampede(mut w: Worker<Box<isize>>,
                s: Stealer<Box<isize>>,
                nthreads: isize,
                amt: usize) {
        for _ in 0..amt {
            w.push(Box::new(20));
        }
        let remaining = Arc::new(AtomicUsize::new(amt));

        let threads = (0..nthreads).map(|_| {
            let remaining = remaining.clone();
            let s = s.clone();
            thread::spawn(move || {
                while remaining.load(SeqCst) > 0 {
                    match s.steal() {
                        Steal::Data(val) => {
                            if *val == 20 {
                                remaining.fetch_sub(1, SeqCst);
                            } else {
                                panic!()
                            }
                        }
                        Steal::Abort | Steal::Empty => {}
                    }
                }
            })
        }).collect::<Vec<_>>();

        while remaining.load(SeqCst) > 0 {
            if let Some(val) = w.try_pop() {
                if *val == 20 {
                    remaining.fetch_sub(1, SeqCst);
                } else {
                    panic!()
                }
            }
        }

        for thread in threads.into_iter() {
            thread.join().unwrap();
        }
    }

    #[test]
    fn run_stampede() {
        let (w, s) = deque();
        stampede(w, s, 8, 10000);
    }

    #[test]
    fn many_stampede() {
        static AMT: usize = 4;
        let threads = (0..AMT).map(|_| {
            let (w, s) = deque();
            thread::spawn(|| {
                stampede(w, s, 4, 10000);
            })
        }).collect::<Vec<_>>();

        for thread in threads.into_iter() {
            thread.join().unwrap();
        }
    }

    #[test]
    fn stress() {
        static AMT: isize = 100000;
        static NTHREADS: isize = 8;
        static DONE: AtomicBool = ATOMIC_BOOL_INIT;
        static HITS: AtomicUsize = ATOMIC_USIZE_INIT;
        let (mut w, s) = deque();

        let threads = (0..NTHREADS).map(|_| {
            let s = s.clone();
            thread::spawn(move || {
                loop {
                    match s.steal() {
                        Steal::Data(2) => { HITS.fetch_add(1, SeqCst); }
                        Steal::Data(..) => panic!(),
                        _ if DONE.load(SeqCst) => break,
                        _ => {}
                    }
                }
            })
        }).collect::<Vec<_>>();

        let mut rng = rand::thread_rng();
        let mut expected = 0;
        while expected < AMT {
            if rng.gen_range(0, 3) == 2 {
                match w.try_pop() {
                    None => {}
                    Some(2) => { HITS.fetch_add(1, SeqCst); },
                    Some(_) => panic!(),
                }
            } else {
                expected += 1;
                w.push(2);
            }
        }

        while HITS.load(SeqCst) < AMT as usize {
            match w.try_pop() {
                None => {}
                Some(2) => { HITS.fetch_add(1, SeqCst); },
                Some(_) => panic!(),
            }
        }
        DONE.store(true, SeqCst);

        for thread in threads.into_iter() {
            thread.join().unwrap();
        }

        assert_eq!(HITS.load(SeqCst), expected as usize);
    }

    #[test]
    fn no_starvation() {
        static AMT: isize = 10000;
        static NTHREADS: isize = 4;
        static DONE: AtomicBool = ATOMIC_BOOL_INIT;
        let (mut w, s) = deque();

        let (threads, hits): (Vec<_>, Vec<_>) = (0..NTHREADS).map(|_| {
            let s = s.clone();
            let ctr = Arc::new(AtomicUsize::new(0));
            let ctr2 = ctr.clone();
            (thread::spawn(move || {
                loop {
                    match s.steal() {
                        Steal::Data((1, 2)) => { ctr.fetch_add(1, SeqCst); }
                        Steal::Data(..) => panic!(),
                        _ if DONE.load(SeqCst) => break,
                        _ => {}
                    }
                }
            }), ctr2)
        }).unzip();

        let mut rng = rand::thread_rng();
        let mut myhit = false;
        'outer: loop {
            for _ in 0..rng.gen_range(0, AMT) {
                if !myhit && rng.gen_range(0, 3) == 2 {
                    match w.try_pop() {
                        None => {}
                        Some((1, 2)) => myhit = true,
                        Some(_) => panic!(),
                    }
                } else {
                    w.push((1, 2));
                }
            }

            for slot in hits.iter() {
                let amt = slot.load(SeqCst);
                if amt == 0 { continue 'outer; }
            }
            if myhit {
                break
            }
        }

        DONE.store(true, SeqCst);

        for thread in threads.into_iter() {
            thread.join().unwrap();
        }
    }
}
