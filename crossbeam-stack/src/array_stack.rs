//! The implementation is inspired by [`crossbeam-queue`](https://crates.io/crates/crossbeam-queue).
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::fmt;
use core::mem::{self, MaybeUninit};
use core::panic::{RefUnwindSafe, UnwindSafe};
use core::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_utils::{Backoff, CachePadded};

/// A slot in a stack.
#[repr(transparent)]
struct Slot<T> {
    /// The value in this slot.
    value: UnsafeCell<MaybeUninit<T>>,
}

/// A bounded stack.
///
/// This stack allocates a fixed-capacity buffer on construction, which is used to store pushed
/// elements. The stack cannot hold more elements than the buffer allows. Attempting to push an
/// element into a full stack will fail.
///
///
/// # Examples
///
/// ```
/// use crossbeam_stack::ArrayStack;
///
/// let q = ArrayStack::new(2);
///
/// assert_eq!(q.push('a'), Ok(()));
/// assert_eq!(q.push('b'), Ok(()));
/// assert_eq!(q.push('c'), Err('c'));
/// assert_eq!(q.pop(), Some('b'));
/// ```
pub struct ArrayStack<T> {
    /// The top of the stack.
    top: CachePadded<AtomicUsize>,

    /// The buffer holding slots.
    buffer: Box<[Slot<T>]>,
}

unsafe impl<T: Send> Sync for ArrayStack<T> {}
unsafe impl<T: Send> Send for ArrayStack<T> {}

impl<T> UnwindSafe for ArrayStack<T> {}
impl<T> RefUnwindSafe for ArrayStack<T> {}

impl<T> ArrayStack<T> {
    /// Creates a new bounded stack with the given capacity.
    ///
    /// # Panics
    ///
    /// Panics if the capacity is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::<i32>::new(100);
    /// ```
    pub fn new(cap: usize) -> Self {
        assert!(cap > 0, "capacity must be non-zero");

        let top = 0;

        // Allocate a buffer of `cap` slots.
        let buffer: Box<[Slot<T>]> = (0..cap)
            .map(|_| Slot {
                value: UnsafeCell::new(MaybeUninit::uninit()),
            })
            .collect();

        Self {
            buffer,
            top: CachePadded::new(AtomicUsize::new(top)),
        }
    }

    /// Attempts to push an element into the stack.
    ///
    /// If the stack is full, the element is returned back as an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::new(1);
    ///
    /// assert_eq!(q.push(10), Ok(()));
    /// assert_eq!(q.push(20), Err(20));
    /// ```
    pub fn push(&self, value: T) -> Result<(), T> {
        let backoff = Backoff::new();

        loop {
            let top = self.top.load(Ordering::Acquire);
            if top >= self.buffer.len() {
                return Err(value);
            }

            if self
                .top
                .compare_exchange_weak(top, top + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // SAFETY: We are the only thread accessing this slot.
                unsafe {
                    self.buffer[top].value.get().write(MaybeUninit::new(value));
                }
                return Ok(());
            }

            backoff.spin();
        }
    }

    /// Attempts to pop an element from the stack.
    ///
    /// If the stack is empty, `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::new(2);
    /// assert_eq!(q.push(10), Ok(()));
    /// assert_eq!(q.push(20), Ok(()));
    ///
    /// assert_eq!(q.pop(), Some(20));
    /// assert_eq!(q.pop(), Some(10));
    /// assert_eq!(q.pop(), None);
    /// ```
    pub fn pop(&self) -> Option<T> {
        let backoff = Backoff::new();

        loop {
            let top = self.top.load(Ordering::Acquire);
            // If the stack is empty, return None.
            if top == 0 {
                return None;
            }

            // Try to decrement the top index.
            if self
                .top
                .compare_exchange_weak(top, top - 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // SAFETY: We are the only thread accessing this slot.
                unsafe {
                    let value = self.buffer[top - 1].value.get().read().assume_init();
                    return Some(value);
                }
            }

            backoff.spin();
        }
    }

    /// Returns the capacity of the stack.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::<i32>::new(100);
    ///
    /// assert_eq!(q.capacity(), 100);
    /// ```
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Returns `true` if the stack is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::new(100);
    ///
    /// assert!(q.is_empty());
    /// q.push(1).unwrap();
    /// assert!(!q.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.top.load(Ordering::SeqCst) == 0
    }

    /// Returns `true` if the stack is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::new(1);
    ///
    /// assert!(!q.is_full());
    /// q.push(1).unwrap();
    /// assert!(q.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.top.load(Ordering::SeqCst) == self.buffer.len()
    }

    /// Returns the number of elements in the stack.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_stack::ArrayStack;
    ///
    /// let q = ArrayStack::new(100);
    /// assert_eq!(q.len(), 0);
    ///
    /// q.push(10).unwrap();
    /// assert_eq!(q.len(), 1);
    ///
    /// q.push(20).unwrap();
    /// assert_eq!(q.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        self.top.load(Ordering::SeqCst)
    }
}

impl<T> Drop for ArrayStack<T> {
    fn drop(&mut self) {
        if mem::needs_drop::<T>() {
            // Get the index of the head.
            let top = *self.top.get_mut();

            // Loop over all slots that hold a message and drop them.
            for i in 0..top {
                unsafe {
                    let slot = self.buffer.get_unchecked_mut(i);
                    (*slot.value.get()).assume_init_drop();
                }
            }
        }
    }
}

impl<T> fmt::Debug for ArrayStack<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("ArrayStack { .. }")
    }
}

impl<T> IntoIterator for ArrayStack<T> {
    type Item = T;

    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { value: self }
    }
}

#[derive(Debug)]
pub struct IntoIter<T> {
    value: ArrayStack<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.value.pop()
    }
}
