//! SPSC ringbuffer

use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::{AtomicUsize, AtomicBool};
use std::sync::Arc;
use std::ptr;
use std::mem;
use std::cmp;
use std::marker::PhantomData;
use mem::CachePadded;

#[inline(always)]
unsafe fn deallocate<T>(ptr: *mut T, old_size: usize) {
    Vec::from_raw_parts(ptr, 0, old_size);
}

#[inline(always)]
unsafe fn allocate<T>(size: usize) -> *mut T {
    let mut buf = Vec::with_capacity(size);
    let ptr = buf.as_mut_ptr();
    mem::forget(buf);
    ptr
}

/// A single-producer, single consumer bounded wait-free ringbuffer queue
///
/// All operations on the buffer queue are wait-free,
/// provided move/copy operations are waitfree.
/// This queue does not allocate after constructions
#[repr(C)]
pub struct SpscBufferQueue<T: Send> {
    // This is a pointer instead of a vector
    // so that vector doesn't call constructors
    data_block: *mut T,
    size: usize,
    _marker: PhantomData<T>,

    // These dummies result in a tremendous performance improvement, ~300%+
    _dummy_1: CachePadded<u64>,
    // data for the consumer
    head: AtomicUsize,
    tail_cache: AtomicUsize,
    prod_alive:AtomicBool, //seems weird, but consumer will read this

    _dummy_2: CachePadded<u64>,
    // data for the producer
    tail: AtomicUsize,
    head_cache: AtomicUsize,
    cons_alive: AtomicBool, //seems weird, but producer will read this
}

unsafe impl<T: Send> Send for SpscBufferQueue<T> {}

impl<T: Send> SpscBufferQueue<T> {
    pub fn new(size: usize) -> (BufferProducer<T>, BufferConsumer<T>) {
        let q = SpscBufferQueue {
            data_block: unsafe{ allocate(size) },
            size: cmp::min(size, (isize::max_value() - 1) as usize) + 1,
            _marker: PhantomData,

            _dummy_1: CachePadded::zeroed(),
            head: AtomicUsize::new(0),
            tail_cache: AtomicUsize::new(0),
            prod_alive: AtomicBool::new(true),

            _dummy_2: CachePadded::zeroed(),
            tail: AtomicUsize::new(0),
            head_cache: AtomicUsize::new(0),
            cons_alive: AtomicBool::new(true),
        };
        let qarc = Arc::new(q);
        let rtuple = (BufferProducer::new(qarc.clone()),
                      BufferConsumer::new(qarc));
        rtuple
    }

    /// Tries constructing the element and inserts into the queue
    ///
    /// Returns the closure if there isn't space
    #[inline(always)]
    fn try_construct<F>(&self, ctor: F) -> Result<(), F> where F: FnOnce() -> T {
        let ctail = self.tail.load(Relaxed);
        let mut next_tail = ctail + 1;
        next_tail = if next_tail == self.size  { 0 } else { next_tail };
        if next_tail == self.head_cache.load(Relaxed) {
            let cur_head = self.head.load(Acquire);
            self.head_cache.store(cur_head, Relaxed);
            if next_tail == cur_head {
                return Err(ctor);
            }
        }
        unsafe {
            let data_pos = self.data_block.offset(ctail as isize);
            ptr::write(data_pos, ctor());
        }
        self.tail.store(next_tail, Release);
        Ok(())
    }

    pub fn try_pop(&self) -> Option<T> {
        let chead = self.head.load(Relaxed);
        if chead == self.tail_cache.load(Relaxed) {
            let cur_tail = self.tail.load(Acquire);
            self.tail_cache.store(cur_tail, Relaxed);
            if chead == cur_tail {
                return None;
            }
        }

        let mut next_head = chead + 1;
        next_head = if next_head == self.size  { 0 } else { next_head };
        unsafe {
            let data_pos = self.data_block.offset(chead as isize);
            let rval = Some(ptr::read(data_pos));
            self.head.store(next_head, Release);
            rval
        }
    }

    pub fn capacity(&self) -> usize {
        self.size - 1 //extra space added in ctor as buffer for head/tail
    }
}


impl<T: Send> Drop for SpscBufferQueue<T> {
    fn drop(&mut self) {
        loop {
            if let None = self.try_pop() {
                break;
            }
        }
        unsafe { deallocate(self.data_block, self.capacity()); }
    }
}

/// The consumer proxy for the SpscBufferQueue
pub struct BufferConsumer<T: Send> {
    spsc: Arc<SpscBufferQueue<T>>,
}

unsafe impl<T: Send> Send for BufferConsumer<T> {}

impl<T: Send> Drop for BufferConsumer<T> {
    fn drop(&mut self) {
        self.spsc.cons_alive.store(false, Release);
    }
}

impl<T: Send> BufferConsumer<T> {
    pub fn new(queue: Arc<SpscBufferQueue<T>>) -> BufferConsumer<T> {
        BufferConsumer {
            spsc: queue,
        }
    }

    /// Creates a new producer if the current one is dead
    pub fn create_producer(&self) -> Option<BufferProducer<T>> {
        if self.spsc.prod_alive.load(Acquire) { return None };
        let rval = Some(BufferProducer::new(self.spsc.clone()));
        self.spsc.prod_alive.store(true, Release);
        rval
    }

    /// Queries whether the producer is currently alive
    #[inline(always)]
    pub fn is_producer_alive(&self) -> bool {
        self.spsc.prod_alive.load(Relaxed)
    }

    /// Attempts to pop an element from the queue
    #[inline(always)]
    pub fn try_pop(&self) -> Option<T> {
        self.spsc.try_pop()
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.spsc.capacity()
    }
}

/// The producer proxy for the SpscBufferQueue
pub struct BufferProducer<T: Send> {
    spsc: Arc<SpscBufferQueue<T>>,
}

unsafe impl<T: Send> Send for BufferProducer<T> {}

impl<T: Send> Drop for BufferProducer<T> {
    fn drop(&mut self) {
        self.spsc.prod_alive.store(false, Release);
    }
}

impl<T: Send> BufferProducer<T> {
    fn new(queue: Arc<SpscBufferQueue<T>>) -> BufferProducer<T> {
        BufferProducer {
            spsc: queue,
        }
    }

    /// Creates a new consumer if the current one is dead
    pub fn create_consumer(&self) -> Option<BufferConsumer<T>> {
        if self.spsc.cons_alive.load(Acquire) { return None }
        let rval = Some(BufferConsumer::new(self.spsc.clone()));
        self.spsc.cons_alive.store(true, Release);
        rval
    }

    /// Queries whether the consumer is currently alive
    #[inline(always)]
    pub fn is_consumer_alive(&self) -> bool {
        self.spsc.cons_alive.load(Relaxed)
    }

    /// Tries pushing the element onto the queue
    ///
    /// Returns an error with the element if the queue is full
    /// or consumer disconnected
    #[inline(always)]
    pub fn try_push(&self, val: T) -> Result<(), T> {
        if !self.is_consumer_alive() {
            return Err(val);
        }
        self.try_construct(|| val).map_err(|f| f())
    }

    /// If there's room in the queue, constructs and inserts an element
    ///
    /// Returns an error with the constructor if the queue is full
    /// or consumer disconnected
    #[inline(always)]
    pub fn try_construct<F>(&self, ctor: F) -> Result<(), F>
        where F: FnOnce() -> T {
        if !self.is_consumer_alive() {
            return Err(ctor);
        }
        self.spsc.try_construct(ctor)
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.spsc.capacity()
    }
}

#[allow(unused_must_use)]
#[cfg(test)]
mod test {

    use scope;
    use super::*;
    use std::sync::atomic::Ordering::{Relaxed};
    use std::sync::atomic::AtomicUsize;
    const CONC_COUNT: i64 = 1000000;

    #[test]
    fn push_pop_1() {
        let (prod, cons) = SpscBufferQueue::<i64>::new(1000);
        assert_eq!(prod.try_push(37), Ok(()));
        assert_eq!(cons.try_pop(), Some(37));
        assert_eq!(cons.try_pop(), None)
    }


    #[test]
    fn push_pop_2() {
        let (prod, cons) = SpscBufferQueue::<i64>::new(1000);
        assert_eq!(prod.try_push(37).is_ok(), true);
        assert_eq!(prod.try_construct(|| 48).is_ok(), true);
        assert_eq!(cons.try_pop(), Some(37));
        assert_eq!(cons.try_pop(), Some(48));
        assert_eq!(cons.try_pop(), None)
    }

    #[test]
    fn push_pop_many_seq() {
        let (prod, cons) = SpscBufferQueue::<i64>::new(1000);
        for i in 0..200 {
            assert_eq!(prod.try_push(i).is_ok(), true);
        }
        for i in 0..200 {
            assert_eq!(cons.try_pop(), Some(i));
        }
    }

    #[test]
    fn push_bounded() {
        let msize = 100;
        let (prod, cons) = SpscBufferQueue::<i64>::new(msize);
        for _ in 0..msize {
            assert_eq!(prod.try_push(1).is_ok(), true);
        }
        assert_eq!(prod.try_push(2), Err(2));
        assert_eq!(cons.try_pop(), Some(1));
        assert_eq!(prod.try_push(2).is_ok(), true);
        for _ in 0..(msize-1) {
            assert_eq!(cons.try_pop(), Some(1));
        }
        assert_eq!(cons.try_pop(), Some(2));

    }

    struct Dropper<'a> {
        aref: &'a AtomicUsize,
    }

    impl<'a> Drop for Dropper<'a> {
        fn drop(& mut self) {
            self.aref.fetch_add(1, Relaxed);
        }
    }

    #[test]
    fn drop_on_dtor() {
        let msize = 100;
        let drop_count = AtomicUsize::new(0);
        {
            let (prod, _) = SpscBufferQueue::<Dropper>::new(msize);
            for _ in 0..msize {
                prod.try_push(Dropper{aref: &drop_count});
            };
        }
        assert_eq!(drop_count.load(Relaxed), msize);
    }

    #[test]
    fn push_pop_many_spsc() {
        let qsize = 100;
        let (prod, cons) = SpscBufferQueue::<i64>::new(qsize);

        scope(|scope| {
            scope.spawn(move || {
                let mut next = 0;

                while next < CONC_COUNT {
                    if let Some(elem) = cons.try_pop() {
                        assert_eq!(elem, next);
                        next += 1;
                    }
                }
            });

            let mut i = 0;
            while i < CONC_COUNT {
                match prod.try_push(i) {
                    Err(_) => continue,
                    Ok(_) => {i += 1;},
                }
            }
        });
    }

    #[test]
    fn test_capacity() {
        let qsize = 100;
        let (prod, cons) = SpscBufferQueue::<i64>::new(qsize);
        assert_eq!(prod.capacity(), qsize);
        assert_eq!(cons.capacity(), qsize);
        for _ in 0..(qsize/2) {
            prod.try_push(1);
        }
        assert_eq!(prod.capacity(), qsize);
        assert_eq!(cons.capacity(), qsize);
    }

    #[test]
    fn test_life_queries() {
        let (prod, cons) = SpscBufferQueue::<i64>::new(1);
        assert_eq!(prod.is_consumer_alive(), true);
        assert_eq!(cons.is_producer_alive(), true);
        assert_eq!(prod.try_push(1), Ok(()));
        {
            let _x = cons;
            assert_eq!(prod.is_consumer_alive(), true);
            assert_eq!(prod.create_consumer().is_none(), true);
        }
        assert_eq!(prod.is_consumer_alive(), false);
        assert_eq!(prod.try_push(1), Err(1));
        let new_cons_o = prod.create_consumer();
        assert_eq!(prod.is_consumer_alive(), true);
        assert_eq!(new_cons_o.is_some(), true);
        assert_eq!(prod.create_consumer().is_none(), true);
        let new_cons = new_cons_o.unwrap();

        {
            let _x = prod;
            assert_eq!(new_cons.is_producer_alive(), true);
            assert_eq!(new_cons.create_producer().is_none(), true);
        }
        assert_eq!(new_cons.is_producer_alive(), false);
        assert_eq!(new_cons.try_pop(), Some(1));
        let new_prod = new_cons.create_producer();
        assert_eq!(new_prod.is_some(), true);
        assert_eq!(new_cons.create_producer().is_none(), true);
    }
}
