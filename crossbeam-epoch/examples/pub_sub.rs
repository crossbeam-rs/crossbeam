use core::sync::atomic::{AtomicIsize, Ordering};
use crossbeam_epoch::{
    arc::{Arc, Atomic, Box},
    pin,
};

static NEW: AtomicIsize = AtomicIsize::new(0);
static CLONE: AtomicIsize = AtomicIsize::new(0);
static DROP: AtomicIsize = AtomicIsize::new(0);

struct Payload {}
impl Payload {
    fn new() -> Payload {
        NEW.fetch_add(1, Ordering::Relaxed);
        Payload {}
    }
}

impl Clone for Payload {
    fn clone(&self) -> Payload {
        CLONE.fetch_add(1, Ordering::Relaxed);
        Payload {}
    }
}

impl Default for Payload {
    fn default() -> Payload {
        Payload::new()
    }
}

impl Drop for Payload {
    fn drop(&mut self) {
        DROP.fetch_add(1, Ordering::Relaxed);
    }
}

struct Node<T> {
    next: Atomic<Option<Arc<Node<T>>>>,
    payload: T,
}

/// Take a basic lock-free queue and add the ability to take an O(1) atomic
/// snapshot of the head.  (Cloning the tail is not interesting; it just makes
/// the tail more stale than usual)
///
/// The resulting structure models an ordered stream of items with multiple
/// publishers and multiple subscribers.  All publishers add items to the same
/// stream; all subscribers consume all items in order at their leisure.
#[derive(Clone)]
struct Publisher<T> {
    tail: Arc<Atomic<Arc<Node<T>>>>,
}

/// A non-atomic single-threaded subscriber would also be useful
struct Subscriber<T> {
    head: Atomic<Arc<Node<T>>>,
}

fn pub_sub_new<T: Default>() -> (Publisher<T>, Subscriber<T>) {
    let sentinel = Arc::new(Node {
        next: Atomic::null(),
        payload: T::default(),
    });
    (
        Publisher {
            tail: Arc::new(Atomic::new(sentinel.clone())),
        },
        Subscriber {
            head: Atomic::new(sentinel.clone()),
        },
    )
}

impl<T> Publisher<T> {
    fn push(&self, x: T) {
        let mut desired = Box::new(Node {
            next: Atomic::null(),
            payload: x,
        });
        let guard = pin();
        let mut tail = unsafe { self.tail.load(Ordering::Acquire, &guard) };
        let mut next = unsafe { tail.next.load(Ordering::Acquire, &guard) };
        loop {
            match next {
                Some(x) => {
                    // must advance stale tail
                    tail = match unsafe {
                        self.tail
                            .compare_exchange_weak(tail, x, Ordering::AcqRel, &guard)
                    } {
                        Ok(_) => x,
                        Err(err) => err.current,
                    };
                    next = unsafe { tail.next.load(Ordering::Acquire, &guard) };
                }
                None => {
                    // try to install the new node
                    match unsafe {
                        tail.next.compare_exchange_weak(
                            Option::<Arc<Node<T>>>::None,
                            desired,
                            Ordering::AcqRel,
                            &guard,
                        )
                    } {
                        Ok(_) => return,
                        Err(err) => {
                            next = err.current;
                            desired = err.desired;
                        }
                    }
                }
            }
        }
    }
}

impl<T: Clone> Subscriber<T> {
    fn pop(&self) -> Option<T> {
        let guard = pin();
        let mut head = unsafe { self.head.load(Ordering::Acquire, &guard) };
        loop {
            match unsafe { head.next.load(Ordering::Relaxed, &guard) } {
                Some(next) => match unsafe {
                    self.head
                        .compare_exchange_weak(head, next, Ordering::AcqRel, &guard)
                } {
                    Ok(_) => break Some(next.payload.clone()),
                    Err(err) => head = err.current,
                },
                None => break None,
            }
        }
    }
}

impl<T> Clone for Subscriber<T> {
    fn clone(&self) -> Self {
        let guard = pin();
        Self {
            head: Atomic::new(unsafe { self.head.load(Ordering::Acquire, &guard) }),
        }
    }
}

fn main() {
    {
        let n = 100;
        let (publisher, subscriber) = pub_sub_new::<Payload>();
        crossbeam_utils::thread::scope(|s| {
            for _ in 0..n {
                s.spawn(|_| {
                    let subscriber = subscriber.clone();
                    publisher.push(Payload::new());
                    for _ in 0..n {
                        subscriber.pop();
                    }
                    // pin.flush();
                });
            }
        })
        .unwrap();
        std::mem::drop(subscriber);
        pin().flush();
    }

    {
        // encourage collection
        let n = 1000;
        for _ in 0..n {
            pin().flush();
        }
    }

    let new = NEW.load(Ordering::Relaxed);
    let clone = CLONE.load(Ordering::Relaxed);
    let drop = DROP.load(Ordering::Relaxed);
    println!(
        "NEW {:?} CLONE {:?} DROP {:?} <= {:?}",
        new,
        clone,
        drop,
        new + clone
    );
    debug_assert!(drop >= 0);
    debug_assert!(drop <= new + clone);
}
