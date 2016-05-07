use std::sync::atomic::Ordering::{Acquire, AcqRel, Release, Relaxed};
use std::sync::atomic::{AtomicBool, AtomicPtr};
use std::{iter, mem, ptr};
use std::thread::{self, Thread};

use mem::epoch::{self, Atomic, Guard, Owned, Shared};
use mem::CachePadded;

/// A Michael-Scott lock-free queue, with support for blocking `pop`s.
///
/// Usable with any number of producers and consumers.
// The representation here is a singly-linked list, with a sentinel
// node at the front. In general the `tail` pointer may lag behind the
// actual tail. Non-sentinal nodes are either all `Data` or all
// `Blocked` (requests for data from blocked threads).
pub struct MsQueue<T> {
    head: CachePadded<Atomic<Node<T>>>,
    tail: CachePadded<Atomic<Node<T>>>,
}

struct Node<T> {
    payload: Payload<T>,
    next: Atomic<Node<T>>,
}

impl<T> Node<T> {
    fn new(p: Payload<T>) -> Node<T> {
        Node { payload: p, next: Atomic::null() }
    }
}

enum Payload<T> {
    /// A node with actual data that can be popped.
    Data(T),
    /// A node representing a blocked request for data.
    Blocked(*mut Signal<T>),
    // A node representing a batch insertion.
    Batch(AtomicPtr<BatchSignal>),
}

/// A blocked request for data, which includes a slot to write the data.
struct Signal<T> {
    /// Thread to unpark when data is ready.
    thread: Thread,
    /// The actual data, when available.
    data: T,
    /// Is the data ready? Needed to cope with spurious wakeups.
    ready: AtomicBool,
}

impl<T> Signal<T> {
    fn signal(signal: *mut Signal<T>, data: T) {
        unsafe {
            // take ownership of the handle. this is needed to avoid
            // accessing memory after we store ready. The woken thread
            // will forget the handle so we are responsible for dropping
            // it.
            let thread = ptr::read(&(*signal).thread);

            // signal the thread
            ptr::write(&mut (*signal).data, data);
            (*signal).ready.store(true, Relaxed);
            thread.unpark();
        }
    }
}

/// A blocked request for batches.
struct BatchSignal {
    /// Thread to unpark when data is ready.
    thread: Thread,
    /// Is the data ready? Needed to cope with spurious wakeups.
    ready: AtomicBool,
}

impl BatchSignal {
    /// Wake up a waiting chane of BatchSignals
    fn wake(ptr: &AtomicPtr<BatchSignal>) {
        // Use the pointer as a well known address to indicate the batch is done
        let dummy = &ptr as *const _ as *mut BatchSignal;

        // Swap with dummy to indicate the batch is done
        let other = ptr.swap(dummy, Relaxed);

        // Wake up the displaced signal.
        if !other.is_null() {
            unsafe {
                // take ownership of the handle. this is needed to avoid accessing
                // memory after we store ready. The woken thread will forget the handle
                // so we are responsible for dropping it.
                let thread = ptr::read(&(*other).thread);
                (*other).ready.store(true, Relaxed);
                thread.unpark();
            }
        }
    }

    /// Wait on a BatchSignal
    fn wait(ptr: &AtomicPtr<BatchSignal>) { BatchSignal::wait_swap(ptr) }

    #[allow(dead_code)]
    fn wait_cas(ptr: &AtomicPtr<BatchSignal>) {
        // Use the pointer as a well known address to indicate the batch is done
        let dummy = &ptr as *const _ as *mut BatchSignal;

        // Setup a signal to wait on.
        let mut signal = BatchSignal {
            thread: thread::current(),
            ready: AtomicBool::new(false),
        };

        // Read the current signal
        let mut other = ptr.load(Relaxed);

        loop {
            // Return if the current state is complete
            if other == dummy { return }

            // Try to swap out the current signal
            let new_signal = ptr.compare_and_swap(other, &mut signal, Relaxed);

            // Break if the swap succeeded
            if other == new_signal { break }

            // Try again
            other = new_signal;
        }

        // Wait to be woken up
        while !signal.ready.load(Relaxed) {
            thread::park();
        }

        // Forget the handle, the thread that woke this one will drop it
        mem::forget(signal.thread);

        // Wake up the displaced signal.
        if !other.is_null() {
            unsafe {
                // take ownership of the handle. this is needed to avoid accessing
                // memory after we store ready. The woken thread will forget the handle
                // so we are responsible for dropping it.
                let thread = ptr::read(&(*other).thread);
                (*other).ready.store(true, Relaxed);
                thread.unpark();
            }
        }
    }
    #[allow(dead_code)]
    fn wait_swap(ptr: &AtomicPtr<BatchSignal>) {
        // Use the pointer as a well known address to indicate the batch is done
        let dummy = ptr as *const _ as *mut BatchSignal;

        // Setup a signal to wait on.
        let mut signal = BatchSignal {
            thread: thread::current(),
            ready: AtomicBool::new(false),
        };

        // Do a swap to check if the batch is done
        let other = ptr.swap(&mut signal, Relaxed);

        // If the batch is done, swap back to the dummy value
        if other == dummy {
            let other = ptr.swap(dummy, Relaxed);

            // Wake up any threads that swapped after the dummy was replaced, but before
            // it was reinstated.
            // It is still necessary to sleep, because signal is being pointed to by
            // some other thread.
            unsafe {
                // take ownership of the handle. this is needed to avoid accessing
                // memory after we store ready. The woken thread will forget the handle
                // so we are responsible for dropping it.
                let thread = ptr::read(&(*other).thread);
                (*other).ready.store(true, Relaxed);
                thread.unpark();
            }
        }

        // Wait to be woken up
        while !signal.ready.load(Relaxed) {
            thread::park();
        }

        // Forget the handle, the thread that woke this one will drop it
        mem::forget(signal.thread);

        // Wake up the displaced signal.
        if other != dummy && other.is_null() {
            unsafe {
                // take ownership of the handle. this is needed to avoid accessing
                // memory after we store ready. The woken thread will forget the handle
                // so we are responsible for dropping it.
                let thread = ptr::read(&(*other).thread);
                (*other).ready.store(true, Relaxed);
                thread.unpark();
            }
        }
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
            head: CachePadded::new(Atomic::new(
                Node::new(unsafe { mem::uninitialized() })
            )),
            tail: CachePadded::new(Atomic::null()),
        };
        let guard = epoch::pin();
        q.tail.store_shared(q.head.load(Relaxed, &guard), Relaxed);
        q
    }

    #[inline(always)]
    /// Get the value of the head pointer
    fn get_head(&self, guard: &Guard) -> *mut Node<T> {
        self.head.load(Relaxed, &guard).unwrap().as_raw()
    }

    #[inline(always)]
    /// Attempt to atomically place `n` into the `next` pointer of `onto`.
    ///
    /// If unsuccessful, returns ownership of `n`, possibly updating
    /// the queue's `tail` pointer.
    fn push_internal(&self,
                     guard: &Guard,
                     onto: Shared<Node<T>>,
                     n: Owned<Node<T>>)
                     -> Result<(), Owned<Node<T>>>
    {
        // try to add the node by swapping the next pointer
        match onto.next.compare_and_swap_ref(None, n, AcqRel, guard) {
            Ok(n) => {
                // success, try to move the tail pointer forward
                self.tail.cas_shared(Some(onto), Some(n.new), Release);
                Ok(())
            },
            Err(n) => {
                // failure, try to "help" by moving the tail pointer forward
                self.tail.cas_shared(Some(onto), n.previous, Release);
                Err(n.new)
            }
        }
    }

    #[inline(always)]
    /// Attempt to atomically place `n` into the `next` pointer of `onto`.
    ///
    /// If unsuccessful, returns ownership of `n`, possibly updating
    /// the queue's `tail` pointer.
    fn push_internal_shared(&self,
                     onto: Shared<Node<T>>,
                     n: Shared<Node<T>>,
                     new_tail: Shared<Node<T>>)
                     -> bool
    {
        // try to swap the tail next pointer
        match onto.next.compare_and_swap_shared(None, Some(n), AcqRel) {
            Ok(_) => {
                // try to move the tail pointer forward
                self.tail.cas_shared(Some(onto), Some(new_tail), Release);
                true
            },
            Err(next) => {
                // try to "help" by moving the tail pointer forward
                self.tail.cas_shared(Some(onto), next, Release);
                false
            },
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
                    Cache::Data(t) => Owned::new(Node::new(Payload::Data(t))),
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

        let mut cache = Cache::Data(t); // don't allocate up front
        let guard = epoch::pin();

        loop {
            // We push onto the tail, so we'll start optimistically by looking
            // there first.
            let tail = self.tail.load(Acquire, &guard).unwrap();

            // Is the queue in Data mode (empty queues can be viewed as either mode)?
            match tail.payload {
                Payload::Batch(ref signal) if self.get_head(&guard) != tail.as_raw() => {
                    BatchSignal::wait(signal);
                    continue
                }
                Payload::Blocked(_) if self.get_head(&guard) != tail.as_raw() => {
                    // Queue is in blocking mode. Attempt to unblock a thread.
                    let head = self.head.load(Acquire, &guard).unwrap();
                    // Get a handle on the first blocked node. Racy, so queue might
                    // be empty or in data mode by the time we see it.
                    let request = head.next.load(Acquire, &guard).and_then(|next| {
                        match next.payload {
                            Payload::Batch(ref signal) => {
                                BatchSignal::wait(signal);
                                None
                            }
                            Payload::Blocked(signal) => Some((next, signal)),
                            Payload::Data(_) => None,
                        }
                    });
                    if let Some((blocked_node, signal)) = request {
                        // race to dequeue the node
                        if self.head.cas_shared(Some(head), Some(blocked_node), Release) {
                            // signal the thread
                            Signal::signal(signal, cache.into_data());
                            unsafe { guard.unlinked(head); }
                            return;
                        }
                    }
                },
                _ => {
                    // Attempt to push onto the `tail` snapshot; fails if
                    // `tail.next` has changed, which will always be the case if the
                    // queue has transitioned to blocking mode.
                    cache = match self.push_internal(&guard, tail, cache.into_node()) {
                        Ok(_) => return,
                        Err(n) => Cache::Node(n), // replace the cache, retry whole thing
                    };
                },
            }
        }
    }

    /// Push several values on contiguously
    pub fn push_batch<I: Iterator<Item=T>>(&self, ts: I) {
        /// We may or may not need to allocate nodes; once we do,
        /// we cache that allocation.
        struct IncompleteCache<'a, T: 'a> {
            head: Atomic<Node<T>>,
            tail: Shared<'a, Node<T>>,
        }

        impl<'a, T: 'a> IncompleteCache<'a, T> {
            /// Construct the IncompleteCache
            fn new(t: T, guard: &'a Guard) -> Self {
                let head = Atomic::new(Node::new(Payload::Data(t)));
                let tail = head.load(Relaxed, guard).unwrap();
                IncompleteCache{
                    head: head,
                    tail: tail,
                }
            }

            /// Add a value to the back of the IncompleteCache
            fn push_back(mut self, t: T, guard: &'a Guard) -> Self {
                self.tail = self.tail.next.store_and_ref(Owned::new(Node::new(Payload::Data(t))), Relaxed, guard);
                self
            }

            /// Add a value to the front of the IncompleteCache
            #[cfg(feature = "nightly")]
            fn push_front(mut self, t: T) -> Self {
                self.head = Atomic::new(Node{
                    payload: Payload::Data(t),
                    next: self.head,
                });
                self
            }

            /// Remove the head element from the cache.
            /// if the cache is empty, it will return None, otherwise it will
            /// return the `self` (after the element has been removed)
            fn pop_front(self, guard: &'a Guard) -> (Option<Self>, T) {
                let head = self.head.load(Relaxed, guard).unwrap();

                // grab the value
                let t = match head.payload {
                    Payload::Data(ref t) => unsafe { ptr::read(t) },
                    _ => unreachable!(),
                };

                // grab the next
                let next = head.next.load(Relaxed, guard);

                // unlink the head
                unsafe { guard.unlinked(head); }

                if let Some(next) = next {
                    self.head.store_shared(Some(next), Relaxed);
                    (Some(self), t)
                } else {
                    (None, t)
                }
            }

            /// Turn the cache into a complete cache
            fn complete(self, guard: &'a Guard) -> CompleteCache<'a, T> {
                CompleteCache{
                    head: self.head.load(Relaxed, guard).unwrap(),
                    tail: self.tail,
                }
            }
        }

        /// A complete cache of data nodes
        struct CompleteCache<'a, T: 'a> {
            head: Shared<'a, Node<T>>,
            tail: Shared<'a, Node<T>>,
        }

        impl<'a, T: 'a> CompleteCache<'a, T> {
            /// Remove the head element from the cache.
            /// if the cache is empty, it will return None, otherwise it will
            /// return the `self` (after the element has been removed)
            fn pop_front(mut self, guard: &'a Guard) -> (Option<Self>, T) {
                // grab the value from the payload
                let t = match self.head.payload {
                    Payload::Data(ref t) => unsafe { ptr::read(t) },
                    _ => unreachable!(),
                };

                // grab the next
                let next = self.head.next.load(Relaxed, &guard);

                // unlink the head
                unsafe { guard.unlinked(self.head); }

                if let Some(next) = next {
                    self.head = next;
                    (Some(self), t)
                } else {
                    (None, t)
                }
            }
        }

        trait DataCacheTrait<'a, T: 'a, I: Iterator<Item=T>>: Sized {
            /// Create a new data cache
            fn new(ts: I) -> Self;

            /// Returns the complete cache if available, othewise adds one item
            /// into the cache and returns that.
            fn complete(self, guard: &'a Guard) -> Result<CompleteCache<'a, T>, Self>;

            /// If `prev` has a node following it, and the cache contains a value,
            /// signal that node with the value. If the `prev` does not have a node
            /// following it, and the cache contains a value, attempt to link in the complete
            /// cache as data nodes.
            // Returns the cache, if it has not been used up, and the last node signalled.
            fn signal(self,
                      guard: &'a Guard,
                      prev: Shared<'a, Node<T>>,
                      tail: &Atomic<Node<T>>)
                      -> (Option<Self>, Shared<'a, Node<T>>);
        }

        enum DataCache<'a, T: 'a, I: Iterator<Item=T>> {
            Empty(I),
            Incomplete(I, IncompleteCache<'a, T>),
            Complete(CompleteCache<'a, T>),
        }

        impl<'a, T: 'a, I: Iterator<Item=T>> DataCache<'a, T, I> {
            /// Returns the complete cache if available, othewise adds one item
            /// into the cache and returns that.
            fn complete_helper(self, guard: &'a Guard) -> Result<CompleteCache<'a, T>, Self> {
                match self {
                    // take one item from the iterator and return an incomplete cache
                    DataCache::Empty(mut iter) => {
                        let t = iter.next().unwrap();
                        Err(DataCache::Incomplete(iter, IncompleteCache::new(t, guard)))
                    },
                    // if there are more items, add one, otherwise return the complete cache
                    DataCache::Incomplete(mut iter, icomp) => {
                        if let Some(t) = iter.next() {
                            Err(DataCache::Incomplete(iter, icomp.push_back(t, guard)))
                        } else {
                            Ok(icomp.complete(guard))
                        }
                    },
                    // return the complete cache
                    DataCache::Complete(comp) => Ok(comp),
                }
            }

            /// If `prev` has a node following it, and the cache contains a value,
            /// signal that node with the value. If the `prev` does not have a node
            /// following it, and the cache contains a value, attempt to link in the complete
            /// cache as data nodes.
            // Returns the cache, if it has not been used up, and the last node signalled.
            fn signal_helper(self,
                             guard: &'a Guard,
                             prev: Shared<'a, Node<T>>,
                             tail: &Atomic<Node<T>>)
                             -> (Option<Self>, Shared<'a, Node<T>>) {
                let node = prev.next.load(Acquire, &guard);
                // Get the node-value pair (along with the cache) if possible,
                // otherwise get the complete cache.
                let result = match (self, node) {
                    (DataCache::Empty(mut iter), Some(node)) => {
                        // extract the next item from the iterator if possible
                        let t = if let Some(t) = iter.next() { t }
                        else { return (None, prev) };

                        Ok((Some(DataCache::Empty(iter)), t, node))
                    },
                    // since there's no node to put values in, just return the more complete cache
                    (DataCache::Empty(mut iter), None) => return (
                        iter.next().map(|t|
                            DataCache::Incomplete(iter, IncompleteCache::new(t, guard))
                        ),
                        prev,
                    ),
                    (DataCache::Incomplete(iter, icomp), Some(node)) => {
                        // extract the next item from the cache
                        let (icomp, t) = icomp.pop_front(guard);
                        if let Some(icomp) = icomp {
                            Ok((Some(DataCache::Incomplete(iter, icomp)), t, node))
                        } else {
                            Ok((Some(DataCache::Empty(iter)), t, node))
                        }
                    },
                    (DataCache::Incomplete(mut iter, icomp), None) => {
                        // If the cache is not complete, just return the more complete cache
                        if let Some(t) = iter.next() {
                            return (
                                Some(DataCache::Incomplete(iter, icomp.push_back(t, guard))),
                                prev,
                            );
                        }

                        Err(icomp.complete(guard))
                    },
                    (DataCache::Complete(comp), Some(node)) => {
                        // extract the next item from the cache
                        let (comp, t) = comp.pop_front(guard);
                        Ok((comp.map(DataCache::Complete), t, node))
                    },
                    (DataCache::Complete(comp), None) => Err(comp),
                };

                // If we have no node, try to swap in our data nodes
                let (cache, t, node) = match result {
                    Ok(v) => v,
                    // try to swap in the data
                    Err(comp) => match prev.next.compare_and_swap_shared(None, Some(comp.head), AcqRel) {
                        Err(next) => {
                            // if we failed, then deal with the node that beat us
                            let (comp, t) = comp.pop_front(guard);
                            (comp.map(DataCache::Complete), t, next.unwrap())
                        },
                        Ok(next) => {
                            // update the tail
                            tail.cas_shared(next, Some(comp.tail), Release);
                            return (None, prev);
                        },
                    },
                };

                // signal the node
                let signal = match node.payload {
                    Payload::Blocked(signal) => signal,
                    _ => unreachable!(),
                };
                Signal::signal(signal, t);

                // unlink the node
                unsafe { guard.unlinked(prev); }

                (cache, node)
            }
        }

        impl<'a, T: 'a, I> DataCacheTrait<'a, T, I> for DataCache<'a, T, I>
        where I: Iterator<Item=T> {
            /// Create a new data cache
            fn new(ts: I) -> Self { DataCache::Empty(ts) }

            /// Returns the complete cache if available, othewise adds one item
            /// into the cache and returns that.
            #[cfg(not(feature = "nightly"))]
            fn complete(self, guard: &'a Guard) -> Result<CompleteCache<'a, T>, Self> {
                self.complete_helper(guard)
            }

            /// Returns the complete cache if available, othewise adds one item
            /// into the cache and returns that.
            #[cfg(feature = "nightly")]
            default fn complete(self, guard: &'a Guard) -> Result<CompleteCache<'a, T>, Self> {
                self.complete_helper(guard)
            }

            /// If `prev` has a node following it, and the cache contains a value,
            /// signal that node with the value. If the `prev` does not have a node
            /// following it, and the cache contains a value, attempt to link in the complete
            /// cache as data nodes.
            // Returns the cache, if it has not been used up, and the last node signalled.
            #[cfg(not(feature = "nightly"))]
            fn signal(self,
                      guard: &'a Guard,
                      prev: Shared<'a, Node<T>>,
                      tail: &Atomic<Node<T>>)
                      -> (Option<Self>, Shared<'a, Node<T>>) {
                self.signal_helper(guard, prev, tail)
            }

            /// If `prev` has a node following it, and the cache contains a value,
            /// signal that node with the value. If the `prev` does not have a node
            /// following it, and the cache contains a value, attempt to link in the complete
            /// cache as data nodes.
            // Returns the cache, if it has not been used up, and the last node signalled.
            #[cfg(feature = "nightly")]
            default fn signal(self,
                      guard: &'a Guard,
                      prev: Shared<'a, Node<T>>,
                      tail: &Atomic<Node<T>>)
                      -> (Option<Self>, Shared<'a, Node<T>>) {
                self.signal_helper(guard, prev, tail)
            }
        }

        /// Specialize for DoubleEndedIterators
        #[cfg(feature = "nightly")]
        impl<'a, T: 'a, I> DataCacheTrait<'a, T, I> for DataCache<'a, T, I>
        where I: Iterator<Item=T>+DoubleEndedIterator {
            /// Returns the complete cache if available, othewise adds one item
            /// into the cache and returns that.
            fn complete(self, guard: &'a Guard) -> Result<CompleteCache<'a, T>, Self> {
                match self {
                    DataCache::Empty(mut iter) => {
                        let t = iter.next_back().unwrap();
                        Err(DataCache::Incomplete(iter, IncompleteCache::new(t, guard)))
                    },
                    DataCache::Incomplete(mut iter, icomp) => {
                        if let Some(t) = iter.next_back() {
                            Err(DataCache::Incomplete(iter, icomp.push_front(t)))
                        } else {
                            Ok(icomp.complete(guard))
                        }
                    },
                    DataCache::Complete(comp) => Ok(comp),
                }
            }

            /// If `prev` has a node following it, and the cache contains a value,
            /// signal that node with the value. If the `prev` does not have a node
            /// following it, and the cache contains a value, attempt to link in the complete
            /// cache as data nodes.
            // Returns the cache, if it has not been used up, and the last node signalled.
            fn signal(self,
                      guard: &'a Guard,
                      prev: Shared<'a, Node<T>>,
                      tail: &Atomic<Node<T>>)
                      -> (Option<Self>, Shared<'a, Node<T>>) {
                let node = prev.next.load(Acquire, &guard);
                // Get the node-value pair (along with the cache) if possible,
                // otherwise get the complete cache.
                let result = match (self, node) {
                    (DataCache::Empty(mut iter), Some(node)) => {
                        // extract the next item from the iterator if possible
                        let t = if let Some(t) = iter.next() { t }
                        else { return (None, prev) };

                        Ok((Some(DataCache::Empty(iter)), t, node))
                    },
                    // since there's no node to put values in, just return the more complete cache
                    (DataCache::Empty(mut iter), None) => return (
                        iter.next_back().map(|t|
                            DataCache::Incomplete(iter, IncompleteCache::new(t, guard))
                        ),
                        prev,
                    ),
                    (DataCache::Incomplete(mut iter, icomp), Some(node)) => {
                        // extract the next item from the iterator if possible,
                        // otherwise extract from the cache (which is now complete)
                        if let Some(t) = iter.next() {
                            Ok((Some(DataCache::Incomplete(iter, icomp)), t, node))
                        } else {
                            let (icomp, t) = icomp.pop_front(guard);
                            Ok((icomp.map(|c| DataCache::Incomplete(iter, c)), t, node))
                        }

                    },
                    (DataCache::Incomplete(mut iter, icomp), None) => {
                        // If the cache is not complete, just return the more complete cache
                        if let Some(t) = iter.next_back() {
                            return (
                                Some(DataCache::Incomplete(iter, icomp.push_front(t))),
                                prev,
                            );
                        }

                        Err(icomp.complete(guard))
                    },
                    (DataCache::Complete(comp), Some(node)) => {
                        // extract the next item from the cache
                        let (comp, t) = comp.pop_front(guard);
                        Ok((comp.map(DataCache::Complete), t, node))
                    },
                    (DataCache::Complete(comp), None) => Err(comp),
                };

                // If we have no node, try to swap in our data nodes
                let (cache, t, node) = match result {
                    Ok(v) => v,
                    // try to swap in the data
                    Err(comp) => match prev.next.compare_and_swap_shared(None, Some(comp.head), AcqRel) {
                        Err(next) => {
                            // if we failed, then deal with the node that beat us
                            let (comp, t) = comp.pop_front(guard);
                            (comp.map(DataCache::Complete), t, next.unwrap())
                        },
                        Ok(next) => {
                            // update the tail
                            tail.cas_shared(next, Some(comp.tail), Release);
                            return (None, prev);
                        },
                    },
                };

                // signal the node
                let signal = match node.payload {
                    Payload::Blocked(signal) => signal,
                    _ => unreachable!(),
                };
                Signal::signal(signal, t);

                // unlink the node
                unsafe { guard.unlinked(prev); }

                (cache, node)
            }
        }

        let mut ts = ts.fuse();
        let ts = match (ts.next(), ts.next()) {
            // if ts has 2+ values, proceed with batch push.
            (Some(t1), Some(t2)) => iter::once(t1).chain(iter::once(t2)).chain(ts),
            // if ts has 1 value, use the normal push
            (Some(t), None) => return self.push(t),
            // if ts is empty, just return
            _ => return,
        };

        let guard = epoch::pin();
        let mut data_cache = DataCache::Empty(ts);

        // Don't allocate the batch nodes upfront
        let mut batch_cache: Option<Owned<Node<T>>> = None;

        loop {
            // We push onto the tail, so we'll start optimistically by looking
            // there first.
            let tail = self.tail.load(Acquire, &guard).unwrap();

            match tail.payload {
                Payload::Batch(ref signal) => BatchSignal::wait(signal),
                Payload::Blocked(_) if self.get_head(&guard) != tail.as_raw() => {
                    // Queue is in blocking mode. Attempt to unblock a thread.
                    let head = self.head.load(Acquire, &guard).unwrap();

                    // Get a handle on the first blocked node. Racy, so queue might
                    // be empty or in data mode by the time we see it.
                    let next = head.next.load(Acquire, &guard);
                    match next.map(|next| &next.payload) {
                        Some(&Payload::Batch(ref signal)) => {
                            BatchSignal::wait(signal);
                            continue
                        },
                        Some(&Payload::Blocked(_)) => {},
                        Some(&Payload::Data(_)) => continue,
                        None => continue,
                    }

                    // setup a batch sentinel
                    let sentinel = match batch_cache {
                        None => {
                            let mut sentinel = Owned::new(Node{
                                payload: Payload::Batch(AtomicPtr::default()),
                                next: Atomic::null(),
                            });
                            // Have the sentinel point to itself, that way it's both head
                            // and payload
                            sentinel.next = unsafe { Atomic::from_raw(&mut *sentinel) } ;
                            sentinel
                        },
                        Some(sentinel) => sentinel,
                    };

                    // swap out the head for the batch sentinel
                    let sentinel = match self.head.cas_and_ref(Some(head), sentinel, AcqRel, &guard) {
                        Ok(sentinel) => sentinel,
                        // if the head has changed then we need to try again from scratch
                        Err(sentinel) => {
                            batch_cache = Some(sentinel);
                            continue;
                        }
                    };

                    //  loop through blocked
                    let mut head = head;
                    loop { match data_cache.signal(&guard, head, &self.tail) {
                        (None, node) => {
                            // assign the new head
                            self.head.store_shared(Some(node), Release);
                            break;
                        },
                        (Some(cache), node) => {
                            data_cache = cache;
                            head = node;
                        },
                    }}

                    // signal all sleeping threads
                    match sentinel.payload {
                        Payload::Batch(ref signal) => BatchSignal::wake(signal),
                        _ => unreachable!(),
                    }

                    // release the batch sentinel
                    unsafe { guard.unlinked(sentinel); }

                    return;
                },
                _ => {
                    // check if the cache if completely formed
                    data_cache = match data_cache.complete(&guard) {
                        Err(data_cache) => data_cache,
                        // Attempt to push the nodes onto the end
                        Ok(comp) => if self.push_internal_shared(tail, comp.head, comp.tail) {
                            // return if our push succeeded
                            return;
                        } else {
                            DataCache::Complete(comp)
                        },
                    }
                },
            }
        }
    }

    #[inline(always)]
    // Attempt to pop a data node. `Ok(None)` if queue is empty or in blocking
    // mode; `Err(())` if lost race to pop.
    fn pop_internal(&self, guard: &Guard) -> Result<Option<T>, ()> {
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
            data: unsafe { mem::uninitialized() },
            ready: AtomicBool::new(false),
        };

        // Go ahead and allocate the blocked node; chances are, we'll need it.
        let mut node = Owned::new(Node::new(Payload::Blocked(&mut signal)));

        loop {
            // try a normal pop
            if let Ok(Some(r)) = self.pop_internal(&guard) {
                // Forget the data, it is uninitialized
                mem::forget(signal.data);

                return r;
            }

            // At this point, we believe the queue is empty/blocked.
            // Snapshot the tail, onto which we want to push a blocked node.
            let tail = self.tail.load(Relaxed, &guard).unwrap();

            // Double-check that we're in blocking mode
            match tail.payload {
                // The current tail is in data mode, so we probably need to abort.
                // BUT, it might be the sentinel, so check for that first.
                Payload::Data(_) if self.get_head(&guard) != tail.as_raw() => continue,
                _ => {
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

                            // Forget the handle, the thread that woke this one will drop it
                            mem::forget(signal.thread);

                            return signal.data;
                        }
                        Err(n) => {
                            node = n;
                        }
                    }
                },
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
    fn push_batch_try_pop_1() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());

        q.push_batch(vec![37].into_iter());
        assert_eq!(q.try_pop(), Some(37));
        assert!(q.is_empty());
    }

    #[test]
    fn push_batch_try_pop_2() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());

        q.push_batch(vec![37,48].into_iter());
        assert_eq!(q.try_pop(), Some(37));
        assert!(!q.is_empty());
        assert_eq!(q.try_pop(), Some(48));
        assert!(q.is_empty());
    }

    #[test]
    fn push_batch_try_pop_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.push_batch(0..200);
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
    fn push_batch_pop_1() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.push_batch(vec![37].into_iter());
        assert!(!q.is_empty());
        assert_eq!(q.pop(), 37);
        assert!(q.is_empty());
    }

    #[test]
    fn push_batch_pop_2() {
        let q: MsQueue<i64> = MsQueue::new();
        q.push_batch(vec![37,48].into_iter());
        assert_eq!(q.pop(), 37);
        assert_eq!(q.pop(), 48);
    }

    #[test]
    fn push_batch_pop_many_seq() {
        let q: MsQueue<i64> = MsQueue::new();
        assert!(q.is_empty());
        q.push_batch(0..200);
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
    fn push_batch_try_pop_many_spsc() {
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

            q.push_batch(0..CONC_COUNT);
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
    fn push_batch_pop_many_spsc() {
        let q: MsQueue<i64> = MsQueue::new();

        scope(|scope| {
            scope.spawn(|| {
                let mut next = 0;
                while next < CONC_COUNT {
                    assert_eq!(q.pop(), next);
                    next += 1;
                }
            });

            q.push_batch(0..CONC_COUNT)
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
