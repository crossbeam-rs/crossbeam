//! A FIFO deque.

use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicIsize, Ordering};
use std::thread;

use epoch::{self, Atomic, Owned};
use utils::cache_padded::CachePadded;

/// Minimum buffer capacity for a deque.
const MIN_CAP: usize = 16;

/// If a buffer of at least this size is retired, thread-local garbage is flushed so that it gets
/// deallocated as soon as possible.
const FLUSH_THRESHOLD_BYTES: usize = 1 << 10;

/// Creates a FIFO deque.
pub fn new<T>() -> (Worker<T>, Stealer<T>) {
    let buffer = Buffer::alloc(MIN_CAP);

    let inner = Arc::new(CachePadded::new(Inner {
        final_back: AtomicIsize::new(0),
        front: AtomicIsize::new(0),
        buffer: Atomic::new(buffer),
    }));

    let w = Worker {
        inner: inner.clone(),
        back: Cell::new(0),
        cached_buffer: Cell::new(buffer),
        _marker: PhantomData,
    };
    let s = Stealer { inner };
    (w, s)
}

/// A slot that may be containing an element.
struct Slot<T> {
    /// The current stamp.
    stamp: AtomicIsize,

    /// The message in this slot.
    value: UnsafeCell<T>,
}

/// A buffer that holds elements in a deque.
///
/// This is just a pointer to the buffer and its length - dropping an instance of this struct will
/// *not* deallocate the buffer.
struct Buffer<T> {
    /// Pointer to the allocated memory.
    ptr: *mut Slot<T>,

    /// Capacity of the buffer. Always a power of two.
    cap: usize,
}

unsafe impl<T> Send for Buffer<T> {}

impl<T> Buffer<T> {
    /// Allocates a new buffer with the specified capacity.
    fn alloc(cap: usize) -> Self {
        debug_assert_eq!(cap, cap.next_power_of_two());

        let mut v = Vec::<Slot<T>>::with_capacity(cap);
        let ptr = v.as_mut_ptr();
        mem::forget(v);

        for i in 0..cap {
            unsafe {
                // Initialize the stamp to an invalid value for this slot.
                let slot = ptr.offset(i as isize);
                let stamp = (i as isize).wrapping_add(1);
                ptr::write(&mut (*slot).stamp, AtomicIsize::new(stamp));
            }
        }

        Buffer { ptr, cap }
    }

    /// Deallocates the buffer.
    unsafe fn dealloc(self) {
        drop(Vec::from_raw_parts(self.ptr, 0, self.cap));
    }

    /// Returns a pointer to the element at the specified `index`.
    unsafe fn at(&self, index: isize) -> *mut Slot<T> {
        // `self.cap` is always a power of two.
        self.ptr.offset(index & (self.cap - 1) as isize)
    }

    /// Writes `value` into the specified `index`.
    unsafe fn write(&self, index: isize, value: T, ord: Ordering) {
        let slot = self.at(index);
        ptr::write((*slot).value.get(), value);
        (*slot).stamp.store(index, ord);
    }

    /// Reads a value from the specified `index`.
    unsafe fn read(&self, index: isize, ord: Ordering) -> Option<T> {
        let slot = self.at(index);
        if (*slot).stamp.load(ord) == index {
            Some(ptr::read((*slot).value.get()))
        } else {
            None
        }
    }

    /// Reads a value from the specified `index`.
    unsafe fn read_unchecked(&self, index: isize) -> T {
        let slot = self.at(index);
        ptr::read((*slot).value.get())
    }

    /// Returns `true` if the slot at `index` contains a value.
    unsafe fn is_ready(&self, index: isize, ord: Ordering) -> bool {
        let slot = self.at(index);
        (*slot).stamp.load(ord) == index
    }
}

impl<T> Clone for Buffer<T> {
    fn clone(&self) -> Buffer<T> {
        Buffer {
            ptr: self.ptr,
            cap: self.cap,
        }
    }
}

impl<T> Copy for Buffer<T> {}

/// Internal data that is shared between the worker and stealers.
///
/// The implementation is based on the following work, but modified for FIFO ordering:
///
/// 1. [Chase and Lev. Dynamic circular work-stealing deque. SPAA 2005.][chase-lev]
/// 2. [Le, Pop, Cohen, and Nardelli. Correct and efficient work-stealing for weak memory models.
///    PPoPP 2013.][weak-mem]
/// 3. [Norris and Demsky. CDSchecker: checking concurrent data structures written with C/C++
///    atomics. OOPSLA 2013.][checker]
///
/// [chase-lev]: https://dl.acm.org/citation.cfm?id=1073974
/// [weak-mem]: https://dl.acm.org/citation.cfm?id=2442524
/// [checker]: https://dl.acm.org/citation.cfm?id=2509514
struct Inner<T> {
    /// The final value of the back index.
    ///
    /// This value will get initialized when the worker gets dropped.
    final_back: AtomicIsize,

    /// The front index.
    front: AtomicIsize,

    /// The underlying buffer.
    buffer: Atomic<Buffer<T>>,
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Load the back index, front index, and buffer.
        let b = self.final_back.load(Ordering::Relaxed);
        let f = self.front.load(Ordering::Relaxed);

        unsafe {
            let buffer = self.buffer.load(Ordering::Relaxed, epoch::unprotected());

            // Go through the buffer from front to back and drop all elements in the deque.
            let mut i = f;
            while i != b {
                ptr::drop_in_place(buffer.deref().at(i));
                i = i.wrapping_add(1);
            }

            // Free the memory allocated by the buffer.
            buffer.into_owned().into_box().dealloc();
        }
    }
}

pub struct Worker<T> {
    /// A reference to the inner representation of the deque.
    inner: Arc<CachePadded<Inner<T>>>,

    /// The back index.
    back: Cell<isize>,

    /// A copy of `inner.buffer` for quick access.
    cached_buffer: Cell<Buffer<T>>,

    /// Indicates that the worker cannot be shared among threads.
    _marker: PhantomData<*mut ()>, // !Send + !Sync
}

unsafe impl<T: Send> Send for Worker<T> {}

impl<T> Worker<T> {
    /// Resizes the internal buffer to the new capacity of `new_cap`.
    #[cold]
    unsafe fn resize(&self, new_cap: usize) {
        // Load the back index, front index, and buffer.
        let b = self.back.get();
        let f = self.inner.front.load(Ordering::Relaxed);
        let buffer = self.cached_buffer.get();

        // Allocate a new buffer.
        let new = Buffer::alloc(new_cap);
        self.cached_buffer.set(new);

        // Copy data from the old buffer to the new one.
        let mut i = f;
        while i != b {
            ptr::copy_nonoverlapping(buffer.at(i), new.at(i), 1);
            i = i.wrapping_add(1);
        }

        let guard = &epoch::pin();

        // Replace the old buffer with the new one.
        let old = self.inner
            .buffer
            .swap(Owned::new(new).into_shared(guard), Ordering::Release, guard);

        // Destroy the old buffer later.
        guard.defer(move || old.into_owned().into_box().dealloc());

        // If the buffer is very large, then flush the thread-local garbage in order to
        // deallocate it as soon as possible.
        if mem::size_of::<Slot<T>>() * new_cap >= FLUSH_THRESHOLD_BYTES {
            guard.flush();
        }
    }

    /// Returns `true` if the deque is empty.
    pub fn is_empty(&self) -> bool {
        let b = self.back.get();
        let f = self.inner.front.load(Ordering::SeqCst);
        b.wrapping_sub(f) <= 0
    }

    /// Pushes an element into the back of the deque.
    pub fn push(&self, value: T) {
        unsafe {
            // Load the back index, front index, and buffer.
            let b = self.back.get();
            let f = self.inner.front.load(Ordering::Acquire);
            let mut buffer = self.cached_buffer.get();

            // Calculate the length and the capacity of the circular buffer.
            let len = b.wrapping_sub(f);

            // Is the deque full?
            if len >= buffer.cap as isize {
                // Yes. Grow the underlying buffer.
                self.resize(2 * buffer.cap);
                buffer = self.cached_buffer.get();
            // Is the new length less than one fourth the capacity?
            } else if buffer.cap > MIN_CAP && len + 1 < buffer.cap as isize / 4 {
                // Yes. Shrink the underlying buffer.
                self.resize(buffer.cap / 2);
                buffer = self.cached_buffer.get();
            }

            // Write `value` into the right slot and increment the back index.
            buffer.write(b, value, Ordering::SeqCst);
            self.back.set(b.wrapping_add(1));
        }
    }

    /// Pops an element from the front of the deque.
    pub fn pop(&self) -> Option<T> {
        // Load the back index.
        let b = self.back.get();

        loop {
            // Load the front index.
            let f = self.inner.front.load(Ordering::Relaxed);

            // Calculate the length of the deque.
            let len = b.wrapping_sub(f);

            // Is the deque empty?
            if len <= 0 {
                return None;
            }

            // Try incrementing the front index to pop the value.
            if self.inner
                .front
                .compare_exchange(f, f.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                unsafe {
                    // Read the value to be popped.
                    let buffer = self.cached_buffer.get();
                    let data = buffer.read_unchecked(f);

                    // Shrink the buffer if `len - 1` is less than one fourth of the capacity.
                    if buffer.cap > MIN_CAP && len <= buffer.cap as isize / 4 {
                        self.resize(buffer.cap / 2);
                    }

                    return Some(data);
                }
            }
        }
    }
}

impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.inner.final_back.store(self.back.get(), Ordering::Relaxed);
    }
}

/// The stealer side of a deque.
pub struct Stealer<T> {
    inner: Arc<CachePadded<Inner<T>>>,
}

unsafe impl<T: Send> Send for Stealer<T> {}
unsafe impl<T: Send> Sync for Stealer<T> {}

impl<T> Stealer<T> {
    /// Returns `true` if the deque is empty.
    pub fn is_empty(&self) -> bool {
        let f = self.inner.front.load(Ordering::Relaxed);

        // A SeqCst fence is needed here.
        //
        // If the current thread is already pinned (reentrantly), we must manually issue the fence.
        // Otherwise, the following pinning will issue the fence anyway, so we don't have have to.
        if epoch::is_pinned() {
            atomic::fence(Ordering::SeqCst);
        }

        let guard = &epoch::pin();

        // Load the buffer.
        let buffer = self.inner.buffer.load(Ordering::Acquire, guard);

        unsafe { buffer.deref().is_ready(f, Ordering::Acquire) }
    }

    /// Steals an element from the front of the deque.
    pub fn steal(&self) -> Option<T> {
        loop {
            // Load the front index.
            let f = self.inner.front.load(Ordering::Acquire);

            // A SeqCst fence is needed here.
            //
            // If the current thread is already pinned (reentrantly), we must manually issue the
            // fence.  Otherwise, the following pinning will issue the fence anyway, so we don't
            // have to.
            if epoch::is_pinned() {
                atomic::fence(Ordering::SeqCst);
            }

            let guard = &epoch::pin();

            // Load the buffer.
            let buffer = self.inner.buffer.load(Ordering::Acquire, guard);

            // Read the value at the front.
            let value = unsafe { buffer.deref().read(f, Ordering::Acquire)? };

            // Try incrementing the front index to steal the value.
            if self.inner
                .front
                .compare_exchange(f, f.wrapping_add(1), Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                return Some(value);
            }

            // We didn't steal this value, forget it.
            mem::forget(value);

            // Yield before retrying.
            drop(guard);
            thread::yield_now();
        }
    }
}

impl<T> Clone for Stealer<T> {
    fn clone(&self) -> Stealer<T> {
        Stealer {
            inner: self.inner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::atomic::Ordering::SeqCst;
    use std::thread;

    use epoch;
    use self::rand::Rng;

    #[test]
    fn smoke() {
        let (w, s) = super::new::<i32>();
        assert_eq!(w.pop(), None);
        assert_eq!(s.steal(), None);

        w.push(1);
        assert_eq!(w.pop(), Some(1));
        assert_eq!(w.pop(), None);
        assert_eq!(s.steal(), None);

        w.push(2);
        assert_eq!(s.steal(), Some(2));
        assert_eq!(s.steal(), None);
        assert_eq!(w.pop(), None);

        w.push(3);
        w.push(4);
        w.push(5);
        assert_eq!(s.steal(), Some(3));
        assert_eq!(s.steal(), Some(4));
        assert_eq!(s.steal(), Some(5));
        assert_eq!(s.steal(), None);

        w.push(6);
        w.push(7);
        w.push(8);
        w.push(9);
        assert_eq!(w.pop(), Some(6));
        assert_eq!(s.steal(), Some(7));
        assert_eq!(w.pop(), Some(8));
        assert_eq!(w.pop(), Some(9));
        assert_eq!(w.pop(), None);

    }

    #[test]
    fn steal_push() {
        const STEPS: usize = 50_000;

        let (w, s) = super::new();
        let t = thread::spawn(move || {
            for i in 0..STEPS {
                loop {
                    if let Some(v) = s.steal() {
                        assert_eq!(i, v);
                        break;
                    }
                }
            }
        });

        for i in 0..STEPS {
            w.push(i);
        }
        t.join().unwrap();
    }

    #[test]
    fn stampede() {
        const COUNT: usize = 50_000;

        let (w, s) = super::new();

        for i in 0..COUNT {
            w.push(Box::new(i + 1));
        }
        let remaining = Arc::new(AtomicUsize::new(COUNT));

        let threads = (0..8)
            .map(|_| {
                let s = s.clone();
                let remaining = remaining.clone();

                thread::spawn(move || {
                    let mut last = 0;
                    while remaining.load(SeqCst) > 0 {
                        if let Some(x) = s.steal() {
                            assert!(last < *x);
                            last = *x;
                            remaining.fetch_sub(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut last = 0;
        while remaining.load(SeqCst) > 0 {
            if let Some(x) = w.pop() {
                assert!(last < *x);
                last = *x;
                remaining.fetch_sub(1, SeqCst);
            }
        }

        for t in threads {
            t.join().unwrap();
        }
    }

    fn run_stress() {
        const COUNT: usize = 50_000;

        let (w, s) = super::new();
        let done = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));

        let threads = (0..8)
            .map(|_| {
                let s = s.clone();
                let done = done.clone();
                let hits = hits.clone();

                thread::spawn(move || {
                    while !done.load(SeqCst) {
                        if let Some(_) = s.steal() {
                            hits.fetch_add(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        let mut rng = rand::thread_rng();
        let mut expected = 0;
        while expected < COUNT {
            if rng.gen_range(0, 3) == 0 {
                if w.pop().is_some() {
                    hits.fetch_add(1, SeqCst);
                }
            } else {
                w.push(expected);
                expected += 1;
            }
        }

        while hits.load(SeqCst) < COUNT {
            if w.pop().is_some() {
                hits.fetch_add(1, SeqCst);
            }
        }
        done.store(true, SeqCst);

        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn stress() {
        run_stress();
    }

    #[test]
    fn stress_pinned() {
        let _guard = epoch::pin();
        run_stress();
    }

    #[test]
    fn no_starvation() {
        const COUNT: usize = 50_000;

        let (w, s) = super::new();
        let done = Arc::new(AtomicBool::new(false));

        let (threads, hits): (Vec<_>, Vec<_>) = (0..8)
            .map(|_| {
                let s = s.clone();
                let done = done.clone();
                let hits = Arc::new(AtomicUsize::new(0));

                let t = {
                    let hits = hits.clone();
                    thread::spawn(move || {
                        while !done.load(SeqCst) {
                            if let Some(_) = s.steal() {
                                hits.fetch_add(1, SeqCst);
                            }
                        }
                    })
                };

                (t, hits)
            })
            .unzip();

        let mut rng = rand::thread_rng();
        let mut my_hits = 0;
        loop {
            for i in 0..rng.gen_range(0, COUNT) {
                if rng.gen_range(0, 3) == 0 && my_hits == 0 {
                    if w.pop().is_some() {
                        my_hits += 1;
                    }
                } else {
                    w.push(i);
                }
            }

            if my_hits > 0 && hits.iter().all(|h| h.load(SeqCst) > 0) {
                break;
            }
        }
        done.store(true, SeqCst);

        for t in threads {
            t.join().unwrap();
        }
    }

    #[test]
    fn destructors() {
        const COUNT: usize = 50_000;

        struct Elem(usize, Arc<Mutex<Vec<usize>>>);

        impl Drop for Elem {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(self.0);
            }
        }

        let (w, s) = super::new();

        let dropped = Arc::new(Mutex::new(Vec::new()));
        let remaining = Arc::new(AtomicUsize::new(COUNT));
        for i in 0..COUNT {
            w.push(Elem(i, dropped.clone()));
        }

        let threads = (0..8)
            .map(|_| {
                let remaining = remaining.clone();
                let s = s.clone();

                thread::spawn(move || {
                    for _ in 0..1000 {
                        if let Some(_) = s.steal() {
                            remaining.fetch_sub(1, SeqCst);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        for _ in 0..1000 {
            if w.pop().is_some() {
                remaining.fetch_sub(1, SeqCst);
            }
        }

        for t in threads {
            t.join().unwrap();
        }

        let rem = remaining.load(SeqCst);
        assert!(rem > 0);

        {
            let mut v = dropped.lock().unwrap();
            assert_eq!(v.len(), COUNT - rem);
            v.clear();
        }

        drop((w, s));

        {
            let mut v = dropped.lock().unwrap();
            assert_eq!(v.len(), rem);
            v.sort();
            for pair in v.windows(2) {
                assert_eq!(pair[0] + 1, pair[1]);
            }
        }
    }
}
