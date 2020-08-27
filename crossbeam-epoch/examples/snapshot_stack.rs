use core::sync::atomic::{AtomicIsize, Ordering};
use crossbeam_epoch::{
    arc::{Arc, Atomic, Box, Redeemable},
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

impl Drop for Payload {
    fn drop(&mut self) {
        DROP.fetch_add(1, Ordering::Relaxed);
    }
}

struct Node<T> {
    next: Option<Arc<Node<T>>>,
    payload: T,
}

impl<T: Clone> Clone for Node<T> {
    fn clone(&self) -> Self {
        Self {
            next: self.next.clone(),
            payload: self.payload.clone(),
        }
    }
}

struct Stack<T> {
    head: Atomic<Option<Arc<Node<T>>>>,
}

impl<T: Clone> Stack<T> {
    fn new() -> Self {
        Stack {
            head: Atomic::default(),
        }
    }

    fn push(&self, value: T) {
        let mut desired = Box::new(Node {
            next: None,
            payload: value,
        });
        let guard = pin();
        let mut expected = unsafe { self.head.load(Ordering::Relaxed, &guard) };
        loop {
            desired.next = Redeemable::redeem(&expected);
            match unsafe {
                self.head.compare_exchange_weak(
                    expected,
                    desired,
                    (Ordering::AcqRel, Ordering::Relaxed),
                    &guard,
                )
            } {
                Ok(_) => break,
                Err(err) => {
                    expected = err.current;
                    desired = err.desired;
                }
            }
        }
    }

    fn pop(&self) -> Option<T> {
        let guard = pin();
        let mut expected = unsafe { self.head.load(Ordering::Acquire, &guard) };
        while let Some(x) = expected {
            let desired = x.next.clone();
            match unsafe {
                self.head.compare_exchange_weak(
                    x,
                    desired,
                    (Ordering::AcqRel, Ordering::Acquire),
                    &guard,
                )
            } {
                Ok(old) => return old.map(|x| x.payload.clone()),
                Err(err) => {
                    expected = err.current;
                }
            }
        }
        None
    }
}

impl<T> Clone for Stack<T> {
    fn clone(&self) -> Self {
        let guard = pin();
        Self {
            head: Atomic::new(unsafe { self.head.load(Ordering::Acquire, &guard) }),
        }
    }
}

fn main() {
    let n = 10000;
    let a = Stack::<Payload>::new();

    crossbeam_utils::thread::scope(|s| {
        for _ in 0..10 {
            s.spawn(|_| {
                for _ in 0..n {
                    a.pop();
                    a.push(Payload::new());
                    //a.pop();
                    //a.pop();
                    //a.push(Payload::new());
                    //a.push(Payload::new());
                    //a.pop();
                    //a.pop();
                    //a.pop();
                }
                // pin.flush();
            });
        }
    })
    .unwrap();
    std::mem::drop(a);

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
