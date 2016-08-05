use std::sync::atomic::Ordering::{Acquire, Release, Relaxed};
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared};
use mem::CachePadded;

/// A Michael-Scott lock-free queue, with support for blocking `pop`s.
///
/// Usable with any number of producers and consumers.
// The representation here is a singly-linked list, with a sentinel
// node at the front. In general the `tail` pointer may lag behind the
// actual tail. Non-sentinal nodes are either all `Data` or all
// `Blocked` (requests for data from blocked threads).
#[derive(Debug)]
pub struct MsQueue<T> {
    head: CachePadded<Atomic<Node<T>>>,
    tail: CachePadded<Atomic<Node<T>>>,
}

#[derive(Debug)]
struct Node<T> {
    payload: Payload<T>,
    next: Atomic<Node<T>>,
}

#[derive(Debug)]
enum Payload<T> {
    /// A node with actual data that can be popped.
    Data(T),
    /// A node representing a blocked request for data.
    Blocked(*mut Signal<T>),
}

/// A blocked request for data, which includes a slot to write the data.
#[derive(Debug)]
struct Signal<T> {
    /// Thread to unpark when data is ready.
    thread: Thread,
    /// The actual data, when available.
    data: Option<T>,
    /// Is the data ready? Needed to cope with spurious wakeups.
    ready: AtomicBool,
}

impl<T> Node<T> {
    fn is_data(&self) -> bool {
        if let Payload::Data(_) = self.payload { true } else { false }
    }
}

// Any particular `T` should never accessed concurrently, so no need
// for Sync.
unsafe impl<T: Send> Sync for MsQueue<T> {}
unsafe impl<T: Send> Send for MsQueue<T> {}

impl<T> MsQueue<T> {
    /// Create a new, empty queue.
    pub fn new() -> MsQueue<T> {
        let q = MsQueue {
            head: CachePadded::new(Atomic::null()),
            tail: CachePadded::new(Atomic::null()),
        };
        let sentinel = Owned::new(Node {
            payload: Payload::Data(unsafe { mem::uninitialized() }),
            next: Atomic::null(),
        });
        let guard = epoch::pin();
        let sentinel = q.head.store_and_ref(sentinel, Relaxed, &guard);
        q.tail.store_shared(Some(sentinel), Relaxed);
        q
    }

    #[inline(always)]
    /// Attempt to atomically place `n` into the `next` pointer of `onto`.
    ///
    /// If unsuccessful, returns ownership of `n`, possibly updating
    /// the queue's `tail` pointer.
    fn push_internal(&self,
                     guard: &epoch::Guard,
                     onto: Shared<Node<T>>,
                     n: Owned<Node<T>>)
                     -> Result<(), Owned<Node<T>>>
    {
        // is `onto` the actual tail?
        if let Some(next) = onto.next.load(Acquire, guard) {
            // if not, try to "help" by moving the tail pointer forward
            self.tail.cas_shared(Some(onto), Some(next), Release);
            Err(n)
        } else {
            // looks like the actual tail; attempt to link in `n`
            onto.next.cas_and_ref(None, n, Release, guard).map(|shared| {
                // try to move the tail pointer forward
                self.tail.cas_shared(Some(onto), Some(shared), Release);
            })
        }
    }

    /// Add `t` to the back of the queue, possibly waking up threads
    /// blocked on `pop`.
    pub fn push(&self, t: T) {
        /// We may or may not need to allocate a node; once we do,
        /// we cache that allocation.
        enum Cache<T> {
            Data(T),
            Node(Owned<Node<T>>),
        }

        impl<T> Cache<T> {
            /// Extract the node if cached, or allocate if not.
            fn into_node(self) -> Owned<Node<T>> {
                match self {
                    Cache::Data(t) => {
                        Owned::new(Node {
                            payload: Payload::Data(t),
                            next: Atomic::null()
                        })
                    }
                    Cache::Node(n) => n
                }
            }

            /// Extract the data from the cache, deallocating any cached node.
            fn into_data(self) -> T {
                match self {
                    Cache::Data(t) => t,
                    Cache::Node(node) => {
                        match node.into_inner().payload {
                            Payload::Data(t) => t,
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }

        let mut cache = Cache::Data(t); // don't allocate up front
        let guard = epoch::pin();

        loop {
            // We push onto the tail, so we'll start optimistically by looking
            // there first.
            let tail = self.tail.load(Acquire, &guard).unwrap();

            // Is the queue in Data mode (empty queues can be viewed as either mode)?
            if tail.is_data() ||
                self.head.load(Relaxed, &guard).unwrap().as_raw() == tail.as_raw()
            {
                // Attempt to push onto the `tail` snapshot; fails if
                // `tail.next` has changed, which will always be the case if the
                // queue has transitioned to blocking mode.
                match self.push_internal(&guard, tail, cache.into_node()) {
                    Ok(_) => return,
                    Err(n) => {
                        // replace the cache, retry whole thing
                        cache = Cache::Node(n)
                    }
                }
            } else {
                // Queue is in blocking mode. Attempt to unblock a thread.
                let head = self.head.load(Acquire, &guard).unwrap();
                // Get a handle on the first blocked node. Racy, so queue might
                // be empty or in data mode by the time we see it.
                let request = head.next.load(Acquire, &guard).and_then(|next| {
                    match next.payload {
                        Payload::Blocked(signal) => Some((next, signal)),
                        Payload::Data(_) => None,
                    }
                });
                if let Some((blocked_node, signal)) = request {
                    // race to dequeue the node
                    if self.head.cas_shared(Some(head), Some(blocked_node), Release) {
                        unsafe {
                            // signal the thread
                            (*signal).data = Some(cache.into_data());
                            (*signal).ready.store(true, Relaxed);
                            (*signal).thread.unpark();
                            guard.unlinked(head);
                            return;
                        }
                    }
                }
            }
        }
    }

    #[inline(always)]
    // Attempt to pop a data node. `Ok(None)` if queue is empty or in blocking
    // mode; `Err(())` if lost race to pop.
    fn pop_internal(&self, guard: &epoch::Guard) -> Result<Option<T>, ()> {
        let head = self.head.load(Acquire, guard).unwrap();
        if let Some(next) = head.next.load(Acquire, guard) {
            if let Payload::Data(ref t) = next.payload {
                unsafe {
                    if self.head.cas_shared(Some(head), Some(next), Release) {
                        guard.unlinked(head);
                        Ok(Some(ptr::read(t)))
                    } else {
                        Err(())
                    }
                }
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    /// Check if this queue is empty.
    pub fn is_empty(&self) -> bool {
        let guard = epoch::pin();
        let head = self.head.load(Acquire, &guard).unwrap();

        if let Some(next) = head.next.load(Acquire, &guard) {
            if let Payload::Data(_) = next.payload {
                false
            } else {
                true
            }
        } else {
            true
        }
    }

    /// Attempt to dequeue from the front.
    ///
    /// Returns `None` if the queue is observed to be empty.
    pub fn try_pop(&self) -> Option<T> {
        let guard = epoch::pin();
        loop {
            if let Ok(r) = self.pop_internal(&guard) {
                return r;
            }
        }
    }

    /// Dequeue an element from the front of the queue, blocking if the queue is
    /// empty.
    pub fn pop(&self) -> T {
        let guard = epoch::pin();

        // Fast path: keep retrying until we observe that the queue has no data,
        // avoiding the allocation of a blocked node.
        loop {
            match self.pop_internal(&guard) {
                Ok(Some(r)) => {
                    return r;
                }
                Ok(None) => {
                    break;
                }
                Err(()) => {}
            }
        }

        // The signal gets to live on the stack, since this stack frame will be
        // blocked until receiving the signal.
        let mut signal = Signal {
            thread: thread::current(),
            data: None,
            ready: AtomicBool::new(false),
        };

        // Go ahead and allocate the blocked node; chances are, we'll need it.
        let mut node = Owned::new(Node {
            payload: Payload::Blocked(&mut signal),
            next: Atomic::null(),
        });

        loop {
            // try a normal pop
            if let Ok(Some(r)) = self.pop_internal(&guard) {
                return r;
            }

            // At this point, we believe the queue is empty/blocked.
            // Snapshot the tail, onto which we want to push a blocked node.
            let tail = self.tail.load(Relaxed, &guard).unwrap();

            // Double-check that we're in blocking mode
            if tail.is_data() {
                // The current tail is in data mode, so we probably need to abort.
                // BUT, it might be the sentinel, so check for that first.
                let head = self.head.load(Relaxed, &guard).unwrap();
                if tail.is_data() && tail.as_raw() != head.as_raw() { continue; }
            }

            // At this point, the tail snapshot is either a blocked node deep in
            // the queue, the sentinel, or no longer accessible from the queue.
            // In *ALL* of these cases, if we succeed in pushing onto the
            // snapshot, we know we are maintaining the core invariant: all
            // reachable, non-sentinel nodes have the same payload mode, in this
            // case, blocked.
            match self.push_internal(&guard, tail, node) {
                Ok(()) => {
                    while !signal.ready.load(Relaxed) {
                        thread::park();
                    }
                    return signal.data.unwrap();
                }
                Err(n) => {
                    node = n;
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use scope;
    use super::*;

    #[test]
    fn push_try_pop_1() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.push(37);
        assert!(!q.is_empty());
        assert_eq!(q.try_pop(), Some(37));
        assert!(q.is_empty());
    }

    #[test]
    fn push_try_pop_2() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.push(37);
        q.push(48);
        assert_eq!(q.try_pop(), Some(37));
        assert!(!q.is_empty());
        assert_eq!(q.try_pop(), Some(48));
        assert!(q.is_empty());
    }

    #[test]
    fn push_try_pop_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        for i in 0..200 {
            q.push(i)
        }
        assert!(!q.is_empty());
        for i in 0..200 {
            assert_eq!(q.try_pop(), Some(i));
        }
        assert!(q.is_empty());
    }

    #[test]
    fn push_pop_1() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.push(37);
        assert!(!q.is_empty());
        assert_eq!(q.pop(), 37);
        assert!(q.is_empty());
    }

    #[test]
    fn push_pop_2() {
        let q: MsQueue<i64> = MsQueue::new();
        q.push(37);
        q.push(48);
        assert_eq!(q.pop(), 37);
        assert_eq!(q.pop(), 48);
    }

    #[test]
    fn push_pop_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        for i in 0..200 {
            q.push(i)
        }
        assert!(!q.is_empty());
        for i in 0..200 {
            assert_eq!(q.pop(), i);
        }
        assert!(q.is_empty());
    }

    #[test]
    fn push_try_pop_many_spsc() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());

        scope(|scope| {
            scope.spawn(|| {
                let mut next = 0;

                while next < CONC_COUNT {
                    if let Some(elem) = q.try_pop() {
                        assert_eq!(elem, next);
                        next += 1;
                    }
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        });
    }

    #[test]
    fn push_try_pop_many_spmc() {
        fn recv(_t: i32, q: &MsQueue<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.try_pop() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 { break }
                }
            }
        }

        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        let qr = &q;
        scope(|scope| {
            for i in 0..3 {
                scope.spawn(move || recv(i, qr));
            }

            scope.spawn(|| {
                for i in 0..CONC_COUNT {
                    q.push(i);
                }
            })
        });
    }

    #[test]
    fn push_try_pop_many_mpmc() {
        enum LR { Left(i64), Right(i64) }

        let q: MsQueue<LR> = MsQueue::new();
        assert!(q.is_empty());

        scope(|scope| {
            for _t in 0..2 {
                scope.spawn(|| {
                    for i in CONC_COUNT-1..CONC_COUNT {
                        q.push(LR::Left(i))
                    }
                });
                scope.spawn(|| {
                    for i in CONC_COUNT-1..CONC_COUNT {
                        q.push(LR::Right(i))
                    }
                });
                scope.spawn(|| {
                    let mut vl = vec![];
                    let mut vr = vec![];
                    for _i in 0..CONC_COUNT {
                        match q.try_pop() {
                            Some(LR::Left(x)) => vl.push(x),
                            Some(LR::Right(x)) => vr.push(x),
                            _ => {}
                        }
                    }

                    let mut vl2 = vl.clone();
                    let mut vr2 = vr.clone();
                    vl2.sort();
                    vr2.sort();

                    assert_eq!(vl, vl2);
                    assert_eq!(vr, vr2);
                });
            }
        });
    }

    #[test]
    fn push_pop_many_spsc() {
        let q: MsQueue<i64> = MsQueue::new();

        scope(|scope| {
            scope.spawn(|| {
                let mut next = 0;
                while next < CONC_COUNT {
                    assert_eq!(q.pop(), next);
                    next += 1;
                }
            });

            for i in 0..CONC_COUNT {
                q.push(i)
            }
        });
        assert!(q.is_empty());
    }

    #[test]
    fn is_empty_dont_pop() {
        let q: MsQueue<i64> = MsQueue::new();
        q.push(20);
        q.push(20);
        assert!(!q.is_empty());
        assert!(!q.is_empty());
        assert!(q.try_pop().is_some());
    }
}
