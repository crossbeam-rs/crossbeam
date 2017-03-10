//! Michael—Scott queues.

use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::{ptr, mem};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Owned, Shared};
use mem::CachePadded;

/// A Michael—Scott lock-free queue.
///
/// This acts as a two-ended list, usable with any number of producers and consumers.
#[derive(Debug)]
pub struct MsQueue<T> {
    /// The sentinel head node.
    ///
    /// Nodes following this might be non-sentinel. Non-sentinel nodes are either all `Data` or
    /// all `Blocked` (requests for data from blocked threads).
    head: CachePadded<Atomic<Node<T>>>,
    /// The tail¹ node.
    ///
    /// ¹In general the pointer may lag behind the actual tail.
    tail: CachePadded<Atomic<Node<T>>>,
}

/// A payload of a node.
///
/// A payload can be in multiple states: It can be blocked by another thread or it can hold some
/// data.
#[derive(Debug)]
enum Payload<T> {
    /// Actual data that can be queued.
    Data(T),
    /// A blocked request for data.
    ///
    /// The data is pending. Eventually, it will become available.
    Blocked(*mut Signal<T>),
}

/// A blocked request for data.
///
/// This includes a slot to write the data.
#[derive(Debug)]
struct Signal<T> {
    /// The thread to unpark when data is ready.
    thread: Thread,
    /// The actual data, when available.
    data: Option<T>,
    /// Is the data ready?
    ///
    /// This is needed to cope with spurious wakeups.
    ready: AtomicBool,
}

/// A node in the list.
#[derive(Debug)]
struct Node<T> {
    /// The payload.
    payload: Payload<T>,
    /// The next node (if any).
    next: Atomic<Node<T>>,
}

impl<T> Node<T> {
    /// Is the data ready and available?
    fn is_data(&self) -> bool {
        if let Payload::Data(_) = self.payload {
            true
        } else {
            false
        }
    }
}

// Any particular `T` should never accessed concurrently, so no need for a `Sync` bound on `T`.
unsafe impl<T: Send> Sync for MsQueue<T> {}
unsafe impl<T: Send> Send for MsQueue<T> {}

impl<T> MsQueue<T> {
    /// Create a new, empty queue.
    pub fn new() -> MsQueue<T> {
        // FIXME: This is ugly.

        // We start setting the two ends to null pointers.
        let queue = MsQueue {
            head: CachePadded::new(Atomic::null()),
            tail: CachePadded::new(Atomic::null()),
        };

        // Construct the sentinel node with empty payload.
        let sentinel = Owned::new(Node {
            payload: Payload::Data(unsafe { mem::uninitialized() }),
            next: Atomic::null(),
        });

        // Set the two ends to the sentinel node.
        let guard = epoch::pin();
        let sentinel = queue.head.store_and_ref(sentinel, atomic::Ordering::Relaxed, &guard);
        queue.tail.store_shared(Some(sentinel), atomic::Ordering::Relaxed);

        queue
    }

    /// Attempt to atomically place `n` into the `next` pointer of `onto`.
    ///
    /// If unsuccessful, returns ownership of `n`, possibly updating the queue's `tail` pointer.
    #[inline(always)]
    fn queue_internal(&self, guard: &epoch::Guard, onto: Shared<Node<T>>, n: Owned<Node<T>>)
        -> Result<(), Owned<Node<T>>> {
        // Is `onto` the actual tail?
        if let Some(next) = onto.next.load(atomic::Ordering::Acquire, guard) {
            // If not, try to "help" by moving the tail pointer forward.
            self.tail.compare_and_set_shared(Some(onto), Some(next), atomic::Ordering::Release);

            Err(n)
        } else {
            // Looks like the actual tail; attempt to link in `n`.
            onto.next.compare_and_set_ref(None, n, atomic::Ordering::Release, guard).map(|shared| {
                // Try to move the tail pointer forward.
                self.tail.compare_and_set_shared(Some(onto), Some(shared), atomic::Ordering::Release);
            })
        }
    }

    /// Push an element to the back of the queue.
    ///
    /// This can possibly waking up threads blocked on `queue`.
    pub fn queue(&self, t: T) {
        // We may or may not need to allocate a node; once we do, we cache that allocation.
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
                            next: Atomic::null(),
                        })
                    }
                    Cache::Node(n) => n,
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

        // We don't allocate up front.
        let mut cache = Cache::Data(t);
        // Pin the epoch.
        let guard = epoch::pin();

        loop {
            // We queue onto the tail, so we'll start optimistically by looking there first.
            let tail = self.tail.load(atomic::Ordering::Acquire, &guard).unwrap();

            // Is the queue in `Data` mode (note that empty queues can be viewed as either mode)?
            if tail.is_data() ||
               self.head.load(atomic::Ordering::Relaxed, &guard).unwrap().as_raw() == tail.as_raw() {
                // Attempt to queue onto the `tail` snapshot; fails if `tail.next` has changed,
                // which will always be the case if the queue has transitioned to blocking mode.
                match self.queue_internal(&guard, tail, cache.into_node()) {
                    Ok(_) => return,
                    Err(n) => {
                        // Replace the cache and retry whole thing.
                        cache = Cache::Node(n)
                    }
                }
            } else {
                // Queue is in blocking mode. Attempt to unblock a thread.
                let head = self.head.load(atomic::Ordering::Acquire, &guard).unwrap();
                // Get a handle on the first blocked node. It is racy, so queue might be empty or
                // in data mode by the time we see it.
                let request = head.next.load(atomic::Ordering::Acquire, &guard).and_then(|next| match next.payload {
                    Payload::Blocked(signal) => Some((next, signal)),
                    Payload::Data(_) => None,
                });
                if let Some((blocked_node, signal)) = request {
                    // Race to dequeue the node.
                    if self.head.compare_and_set_shared(Some(head), Some(blocked_node), atomic::Ordering::Release) {
                        unsafe {
                            // Signal the result to the thread.
                            (*signal).data = Some(cache.into_data());
                            // The data is now ready.
                            (*signal).ready.store(true, atomic::Ordering::Release);
                            // Try to unpark the associated thread.
                            (*signal).thread.unpark();
                            // And finally, unlink the head from the epoch.
                            guard.unlinked(head);

                            return;
                        }
                    }
                }
            }
        }
    }

    // Attempt to dequeue a data node from the front.
    //
    // It returns `Ok(None)` if queue is empty or in blocking mode; `Err(())` if lost race to
    // another `dequeue`.
    #[inline(always)]
    fn dequeue_internal(&self, guard: &epoch::Guard) -> Result<Option<T>, ()> {
        // Load the head (with the provideed guard).
        let head = self.head.load(atomic::Ordering::Acquire, guard).unwrap();
        // Check if the head is empty or not.
        if let Some(next) = head.next.load(atomic::Ordering::Acquire, guard) {
            // The head is not empty; check the payload.
            if let Payload::Data(ref t) = next.payload {
                // There was some data in the payload, attempt to remove the head.
                unsafe {
                    if self.head.compare_and_set_shared(Some(head), Some(next), atomic::Ordering::Release) {
                        // Unlink the head from the epoch so it can eventually be deallocated.
                        // TODO: Is this really necessary?
                        guard.unlinked(head);

                        // As we have exclusive accesss (by the CAS above), we can safely move out
                        // of the pointer.
                        Ok(Some(ptr::read(t)))
                    } else {
                        // The head has been dequeued by another thread. We lost the race.
                        Err(())
                    }
                }
            } else {
                // Nothing in the node.
                Ok(None)
            }
        } else {
            // Yep. Empty.
            Ok(None)
        }
    }

    /// Is this queue empty?
    pub fn is_empty(&self) -> bool {
        let guard = epoch::pin();
        let head = self.head.load(atomic::Ordering::Acquire, &guard).unwrap();

        if let Some(next) = head.next.load(atomic::Ordering::Acquire, &guard) {
            // Check if the payload is empty or not.
            if let Payload::Data(_) = next.payload { false } else { true }
        } else {
            // There were no node after mandatory head node, so the queue is empty.
            true
        }
    }

    /// Attempt to dequeue from the front.
    ///
    /// This returns `None` if the queue is observed to be empty.
    pub fn dequeue(&self) -> Option<T> {
        // Pin the epoch.
        let guard = epoch::pin();
        // Spin until we win a race.
        loop {
            if let Ok(r) = self.dequeue_internal(&guard) {
                return r;
            }
        }
    }

    /// Dequeue an element from the front of the queue, blocking if the queue is empty.
    pub fn dequeue_or_wait(&self) -> T {
        // TODO: Consider using a kernel primitive like futex for avoid wasting cycles. Check out
        //       the `parking_lot` crate.

        // Pin the epoch.
        let guard = epoch::pin();

        // The fast path: keep retrying until we observe that the queue has no data, avoiding the
        // allocation of a blocked node.
        loop {
            match self.dequeue_internal(&guard) {
                Ok(Some(r)) => {
                    return r;
                }
                Ok(None) => {
                    break;
                }
                Err(()) => {}
            }
        }

        // The signal gets to live on the stack, since this stack frame will be blocked until
        // receiving the signal, hence it is valid for the needed period.
        // TODO: Express this through the type system.
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
            // Try a normal dequeue.
            if let Ok(Some(r)) = self.dequeue_internal(&guard) {
                return r;
            }

            // At this point, we believe the queue is empty/blocked. Snapshot the tail, onto which
            // we want to queue a blocked node.
            let tail = self.tail.load(atomic::Ordering::Relaxed, &guard).unwrap();

            // Double-check that we're in blocking mode.
            if tail.is_data() {
                // The current tail is in data mode, so we probably need to abort. But, it might
                // be the sentinel, so check for that first.
                let head = self.head.load(atomic::Ordering::Relaxed, &guard).unwrap();
                if tail.is_data() && tail.as_raw() != head.as_raw() {
                    continue;
                }
            }

            // At this point, the tail snapshot is either a blocked node deep in the queue, the
            // sentinel, or no longer accessible from the queue. In *ALL* of these cases, if we
            // succeed in queuing onto the snapshot, we know we are maintaining the core invariant:
            // all reachable, non-sentinel nodes have the same payload mode, in this case, blocked.
            match self.queue_internal(&guard, tail, node) {
                Ok(()) => {
                    while !signal.ready.load(atomic::Ordering::Acquire) {
                        // The signal is not ready, so we park the thread until then.
                        thread::park();
                    }

                    // As the data is completed, we have something to return.
                    return signal.data.unwrap();
                }
                Err(n) => {
                    // Give back ownership.
                    node = n;
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    const CONC_COUNT: i64 = 1000000;

    use super::*;
    use std::thread;

    #[test]
    fn queue_dequeue_1() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.queue(37);
        assert!(!q.is_empty());
        assert_eq!(q.dequeue(), Some(37));
        assert!(q.is_empty());
    }

    #[test]
    fn queue_dequeue_2() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.queue(37);
        q.queue(48);
        assert_eq!(q.dequeue(), Some(37));
        assert!(!q.is_empty());
        assert_eq!(q.dequeue(), Some(48));
        assert!(q.is_empty());
    }

    #[test]
    fn queue_dequeue_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        for i in 0..200 {
            q.queue(i)
        }
        assert!(!q.is_empty());
        for i in 0..200 {
            assert_eq!(q.dequeue(), Some(i));
        }
        assert!(q.is_empty());
    }

    #[test]
    fn queue_dequeue_or_wait_1() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.queue(37);
        assert!(!q.is_empty());
        assert_eq!(q.dequeue_or_wait(), 37);
        assert!(q.is_empty());
    }

    #[test]
    fn queue_dequeue_or_wait_2() {
        let q: MsQueue<i64> = MsQueue::new();
        q.queue(37);
        q.queue(48);
        assert_eq!(q.dequeue_or_wait(), 37);
        assert_eq!(q.dequeue_or_wait(), 48);
    }

    #[test]
    fn queue_dequeue_or_wait_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        for i in 0..200 {
            q.queue(i)
        }
        assert!(!q.is_empty());
        for i in 0..200 {
            assert_eq!(q.dequeue_or_wait(), i);
        }
        assert!(q.is_empty());
    }

    #[test]
    fn queue_dequeue_many_spsc() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());

        let join = thread::spawn(|| {
            let mut next = 0;

            while next < CONC_COUNT {
                if let Some(elem) = q.dequeue() {
                    assert_eq!(elem, next);
                    next += 1;
                }
            }
        });

        for i in 0..CONC_COUNT {
            q.queue(i)
        }

        join.join().unwrap();
    }

    #[test]
    fn queue_dequeue_many_spmc() {
        fn recv(_t: i32, q: &MsQueue<i64>) {
            let mut cur = -1;
            for _i in 0..CONC_COUNT {
                if let Some(elem) = q.dequeue() {
                    assert!(elem > cur);
                    cur = elem;

                    if cur == CONC_COUNT - 1 {
                        break;
                    }
                }
            }
        }

        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        let qr = &q;
        let mut v = Vec::new();
        for i in 0..3 {
            v.push(thread::spawn(move || recv(i, qr)));
        }

        v.push(thread::spawn(|| for i in 0..CONC_COUNT {
            q.queue(i);
        }));

        for i in v {
            i.join().unwrap();
        }
    }

    #[test]
    fn queue_dequeue_many_mpmc() {
        enum LR {
            Left(i64),
            Right(i64),
        }

        let q: MsQueue<LR> = MsQueue::new();
        assert!(q.is_empty());

        let mut v = Vec::new();

        for _t in 0..2 {
            v.push(thread::spawn(|| for i in CONC_COUNT - 1..CONC_COUNT {
                q.queue(LR::Left(i))
            }));
            v.push(thread::spawn(|| for i in CONC_COUNT - 1..CONC_COUNT {
                q.queue(LR::Right(i))
            }));
            v.push(thread::spawn(|| {
                let mut vl = vec![];
                let mut vr = vec![];
                for _i in 0..CONC_COUNT {
                    match q.dequeue() {
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
            }));
        }

        for i in v {
            i.join().unwrap();
        }
    }

    #[test]
    fn queue_dequeue_or_wait_many_spsc() {
        let q: MsQueue<i64> = MsQueue::new();

        let join = thread::spawn(|| {
            let mut next = 0;
            while next < CONC_COUNT {
                assert_eq!(q.dequeue_or_wait(), next);
                next += 1;
            }
        });

        for i in 0..CONC_COUNT {
            q.queue(i)
        }

        join.join().unwrap();

        assert!(q.is_empty());
    }

    #[test]
    fn is_empty_dont_dequeue() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.queue(20);
        q.queue(20);
        assert!(!q.is_empty());
        assert!(!q.is_empty());
        assert!(q.dequeue().is_some());
        assert!(!q.is_empty());
        assert!(q.dequeue().is_some());
        assert!(q.is_empty());
        assert!(q.dequeue().is_none());
    }
}
